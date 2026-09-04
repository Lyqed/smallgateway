// Package controllers holds the LLMGateway reconciler: the level-triggered,
// idempotent controller that turns an LLMGateway CR into a running gatewayctl
// (control plane) + gatewayd (data planes) joined over gRPC, and writes status
// back to the CR.
//
// Reconcile discipline (standard controller-runtime):
//   - LEVEL-TRIGGERED: every pass reconciles desired-from-observed against the
//     LIVE cluster state (CreateOrUpdate), never a diff of events. A missed
//     event, a manual kubectl edit of a child, or an operator restart all heal
//     on the next pass.
//   - IDEMPOTENT: re-running with no spec change converges to the same objects
//     and makes no writes beyond status.
//   - OWNER REFERENCES: every child carries the CR as owner, so deleting the CR
//     garbage-collects the ConfigMap, Secret, Deployments and Services.
//   - REQUEUE BACKOFF: errors return with error (controller-runtime applies
//     exponential backoff); a healthy-but-not-yet-Ready pass requeues after a
//     fixed short interval. No hot loop.
package controllers

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"time"

	appsv1 "k8s.io/api/apps/v1"
	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	"k8s.io/apimachinery/pkg/api/resource"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/apimachinery/pkg/util/intstr"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/controller/controllerutil"
	"sigs.k8s.io/controller-runtime/pkg/log"

	gwv1 "github.com/Lyqed/smallgateway/deploy/operator/api/v1alpha1"
	"github.com/Lyqed/smallgateway/deploy/operator/internal/render"
)

const (
	// requeueAfterProgress is the requeue delay while children are converging
	// (e.g. pods not yet Ready). Short enough to feel responsive, long enough
	// to avoid a hot loop.
	requeueAfterProgress = 10 * time.Second
	// requeueSteady re-checks a Ready gateway periodically so status tracks
	// data-plane replica changes and drift without waiting on an event.
	requeueSteady = 60 * time.Second

	ctlListenPort   = 6187
	joinTokenKey    = "join-token"
	joinTokenEnvVar = "GATEWAY_JOIN_TOKEN"

	// ctlStatusPort is gatewayctl's read-only status surface (GET /status):
	// per-node ack state + the applied render identity. The operator's Ready
	// gate reads it, so Ready means "the fleet COMMITTED this config", not
	// "the pods look healthy".
	ctlStatusPort = 6186
)

// tokenSelectScript (POSIX sh) derives the pod's ordinal from its stable
// StatefulSet name (…-<ordinal>) and picks its single-use LABEL token:
// ordinal 0 -> the base secret, ordinal N -> "<base>-<N+1>". The operator
// passes one --label-token per replica to gatewayctl with the same suffix
// scheme, so any replica count gets a distinct token — no cap.
const tokenSelectScript = `ORD="${POD_NAME##*-}"
if [ "$ORD" = "0" ]; then
  NODE_TOKEN="$GATEWAY_JOIN_TOKEN"
else
  NODE_TOKEN="${GATEWAY_JOIN_TOKEN}-$((ORD + 1))"
fi
export NODE_TOKEN`

// Reconciler reconciles LLMGateway objects.
type Reconciler struct {
	client.Client
	Scheme *runtimeScheme
	// Default images, overridable per-CR. Set from operator flags.
	DefaultGatewaydImage   string
	DefaultGatewayctlImage string
}

// runtimeScheme is a tiny alias so the struct field name reads clearly; the
// concrete type is *runtime.Scheme, injected in main.go.
type runtimeScheme = schemeAlias

// Reconcile is the level-triggered entry point.
func (r *Reconciler) Reconcile(ctx context.Context, req ctrl.Request) (ctrl.Result, error) {
	l := log.FromContext(ctx)

	var gw gwv1.LLMGateway
	if err := r.Get(ctx, req.NamespacedName, &gw); err != nil {
		// Not found: the CR was deleted; children GC via owner refs. Nothing to do.
		return ctrl.Result{}, client.IgnoreNotFound(err)
	}

	// Deletion is handled by owner-reference garbage collection; no finalizer
	// logic is required because every child is owned by the CR. We keep a
	// no-op finalizer add/remove OUT of M1 to stay minimal — GC is sufficient
	// and avoids a stuck-finalizer failure mode.

	// 1) Render the config-repo fragments from the spec (pure function).
	rendered, err := render.Render(&gw.Spec)
	if err != nil {
		return r.fail(ctx, &gw, "RenderFailed", fmt.Sprintf("rendering config repo: %v", err))
	}

	names := childNames(gw.Name)

	// 2) Ensure the join-token Secret (generated once, then stable).
	joinToken, err := r.ensureJoinSecret(ctx, &gw, names)
	if err != nil {
		return r.fail(ctx, &gw, "SecretFailed", fmt.Sprintf("join-token secret: %v", err))
	}

	// 3) Ensure the config-repo ConfigMap (the gatewayctl --repo mount).
	if err := r.ensureRepoConfigMap(ctx, &gw, names, rendered); err != nil {
		return r.fail(ctx, &gw, "ConfigMapFailed", fmt.Sprintf("config-repo configmap: %v", err))
	}

	// 4) Ensure the control-plane (gatewayctl) Deployment + Service.
	if err := r.ensureControlPlane(ctx, &gw, names, rendered.Hash); err != nil {
		return r.fail(ctx, &gw, "ControlPlaneFailed", fmt.Sprintf("gatewayctl: %v", err))
	}

	// 5) Ensure the data-plane (gatewayd) Deployment + Service, joined to the
	//    control plane over gRPC with the join token.
	if err := r.ensureDataPlanes(ctx, &gw, names); err != nil {
		return r.fail(ctx, &gw, "DataPlaneFailed", fmt.Sprintf("gatewayd: %v", err))
	}
	_ = joinToken // consumed via env-from-secret in the deployments

	// 6) Observe child readiness and write status.
	ctlReady, err := r.deploymentReady(ctx, gw.Namespace, names.ctlDeploy)
	if err != nil {
		return ctrl.Result{}, err
	}
	dpReady, dpDesired, err := r.statefulSetReplicas(ctx, gw.Namespace, names.dpDeploy)
	if err != nil {
		return ctrl.Result{}, err
	}

	gw.Status.ObservedGeneration = gw.Generation
	gw.Status.RenderedConfigHash = rendered.Hash
	gw.Status.ControlPlaneReady = ctlReady
	gw.Status.DataPlanes = fmt.Sprintf("%d/%d", dpReady, dpDesired)

	// 7) The fleet-commit gate: query gatewayctl's status surface for what
	// each node ACTUALLY acked. Ready no longer means "the pods look
	// healthy" — it means every desired node acked the control plane's
	// currently-applied render. This closes the documented rollout window
	// where renderedConfigHash showed a new input hash while the fleet
	// still served the old config.
	childrenReady := ctlReady && dpReady >= 1 && dpReady == dpDesired
	fleetCommitted := false
	fleetMsg := ""
	if childrenReady {
		fs, err := r.queryFleetStatus(ctx, gw.Namespace, names.ctlService)
		if err != nil {
			fleetMsg = fmt.Sprintf("fleet status unavailable: %v", err)
			gw.Status.Nodes = nil
		} else {
			gw.Status.Nodes = fs.nodeStatuses()
			fleetCommitted, fleetMsg = fs.committed(dpDesired)
		}
	}

	ready := childrenReady && fleetCommitted
	if ready {
		setCondition(&gw.Status, "Ready", metav1.ConditionTrue, "FleetCommitted",
			fmt.Sprintf("all %d data plane(s) acked the applied render", dpDesired), gw.Generation)
		setCondition(&gw.Status, "Degraded", metav1.ConditionFalse, "Healthy", "", gw.Generation)
	} else {
		reason := "ControlPlaneNotReady"
		msg := "waiting for gatewayctl to become Ready"
		if ctlReady && !childrenReady {
			reason = "DataPlanesNotReady"
			msg = fmt.Sprintf("waiting for data planes (%d/%d Ready)", dpReady, dpDesired)
		} else if childrenReady {
			reason = "AwaitingFleetCommit"
			msg = fleetMsg
		}
		setCondition(&gw.Status, "Ready", metav1.ConditionFalse, reason, msg, gw.Generation)
	}

	if err := r.Status().Update(ctx, &gw); err != nil {
		return ctrl.Result{}, err
	}

	if !ready {
		l.Info("reconciled; awaiting readiness", "dataPlanes", gw.Status.DataPlanes, "ctlReady", ctlReady)
		return ctrl.Result{RequeueAfter: requeueAfterProgress}, nil
	}
	l.Info("reconciled; Ready", "hash", rendered.Hash[:12], "dataPlanes", gw.Status.DataPlanes)
	return ctrl.Result{RequeueAfter: requeueSteady}, nil
}

// --- child ensure helpers (all CreateOrUpdate = idempotent + level) ---------

func (r *Reconciler) ensureJoinSecret(ctx context.Context, gw *gwv1.LLMGateway, n childNameSet) (string, error) {
	sec := &corev1.Secret{ObjectMeta: metav1.ObjectMeta{Name: n.secret, Namespace: gw.Namespace}}
	var token string
	_, err := controllerutil.CreateOrUpdate(ctx, r.Client, sec, func() error {
		if err := controllerutil.SetControllerReference(gw, sec, r.Scheme); err != nil {
			return err
		}
		applyLabels(&sec.ObjectMeta, gw.Name, "join-token")
		if sec.Data == nil {
			sec.Data = map[string][]byte{}
		}
		// Generate ONCE; keep stable across reconciles so joined nodes are not
		// churned. Only fill if absent.
		if _, ok := sec.Data[joinTokenKey]; !ok {
			buf := make([]byte, 24)
			if _, err := rand.Read(buf); err != nil {
				return err
			}
			sec.Data[joinTokenKey] = []byte(hex.EncodeToString(buf))
		}
		token = string(sec.Data[joinTokenKey])
		return nil
	})
	return token, err
}

func (r *Reconciler) ensureRepoConfigMap(ctx context.Context, gw *gwv1.LLMGateway, n childNameSet, res *render.Result) error {
	cm := &corev1.ConfigMap{ObjectMeta: metav1.ObjectMeta{Name: n.repoCM, Namespace: gw.Namespace}}
	_, err := controllerutil.CreateOrUpdate(ctx, r.Client, cm, func() error {
		if err := controllerutil.SetControllerReference(gw, cm, r.Scheme); err != nil {
			return err
		}
		applyLabels(&cm.ObjectMeta, gw.Name, "config-repo")
		if cm.Annotations == nil {
			cm.Annotations = map[string]string{}
		}
		cm.Annotations["gateway.opensourcegateway.com/config-hash"] = res.Hash
		// Repo fragments become ConfigMap keys with '/' flattened to '__' (a
		// ConfigMap key cannot contain '/'); the init step reconstructs the
		// directory tree into the gatewayctl --repo mount. See the gatewayctl
		// deployment's initContainer.
		cm.Data = map[string]string{}
		for _, f := range res.Fragments {
			cm.Data[flattenPath(f.Path)] = string(f.Bytes)
		}
		return nil
	})
	return err
}

func (r *Reconciler) ensureControlPlane(ctx context.Context, gw *gwv1.LLMGateway, n childNameSet, configHash string) error {
	image := r.DefaultGatewayctlImage
	if gw.Spec.ControlPlane != nil && gw.Spec.ControlPlane.Image != "" {
		image = gw.Spec.ControlPlane.Image
	}

	dep := &appsv1.Deployment{ObjectMeta: metav1.ObjectMeta{Name: n.ctlDeploy, Namespace: gw.Namespace}}
	_, err := controllerutil.CreateOrUpdate(ctx, r.Client, dep, func() error {
		if err := controllerutil.SetControllerReference(gw, dep, r.Scheme); err != nil {
			return err
		}
		applyLabels(&dep.ObjectMeta, gw.Name, "control-plane")
		sel := selectorLabels(gw.Name, "control-plane")
		replicas := int32(1)
		dep.Spec.Replicas = &replicas
		dep.Spec.Selector = &metav1.LabelSelector{MatchLabels: sel}
		dep.Spec.Template.ObjectMeta.Labels = sel
		if dep.Spec.Template.Annotations == nil {
			dep.Spec.Template.Annotations = map[string]string{}
		}
		// Roll the control plane when the rendered config changes.
		dep.Spec.Template.Annotations["gateway.opensourcegateway.com/config-hash"] = configHash

		dep.Spec.Template.Spec.Volumes = []corev1.Volume{
			{Name: "repo-src", VolumeSource: corev1.VolumeSource{
				ConfigMap: &corev1.ConfigMapVolumeSource{LocalObjectReference: corev1.LocalObjectReference{Name: n.repoCM}}}},
			{Name: "repo", VolumeSource: corev1.VolumeSource{EmptyDir: &corev1.EmptyDirVolumeSource{}}},
		}
		// initContainer reconstructs the flattened ConfigMap keys into the
		// repo directory tree gatewayctl --repo reads.
		dep.Spec.Template.Spec.InitContainers = []corev1.Container{{
			Name:    "unpack-repo",
			Image:   image,
			Command: []string{"/bin/sh", "-c", unpackScript},
			VolumeMounts: []corev1.VolumeMount{
				{Name: "repo-src", MountPath: "/repo-src", ReadOnly: true},
				{Name: "repo", MountPath: "/repo"},
			},
		}}
		// The join token is a gatewayctl FLAG; we pass it from the mounted
		// Secret env var through an exec-shell wrapper so the secret value is
		// never embedded in the manifest.
		dep.Spec.Template.Spec.Containers = []corev1.Container{{
			Name:    "gatewayctl",
			Image:   image,
			Command: []string{"/bin/sh", "-c"},
			Args:    []string{ctlCommand(gw)},
			Env: []corev1.EnvVar{{
				Name: joinTokenEnvVar,
				ValueFrom: &corev1.EnvVarSource{SecretKeyRef: &corev1.SecretKeySelector{
					LocalObjectReference: corev1.LocalObjectReference{Name: n.secret},
					Key:                  joinTokenKey,
				}},
			}},
			Ports: []corev1.ContainerPort{
				{Name: "fleet", ContainerPort: ctlListenPort},
				{Name: "status", ContainerPort: ctlStatusPort},
			},
			VolumeMounts:   []corev1.VolumeMount{{Name: "repo", MountPath: "/repo"}},
			LivenessProbe:  tcpProbe("fleet", 5, 10),
			ReadinessProbe: tcpProbe("fleet", 3, 5),
			Resources:      defaultResources("100m", "128Mi", "500m", "256Mi"),
		}}
		return nil
	})
	if err != nil {
		return err
	}

	// Control-plane Service (headless-friendly ClusterIP; data planes dial it).
	svc := &corev1.Service{ObjectMeta: metav1.ObjectMeta{Name: n.ctlService, Namespace: gw.Namespace}}
	_, err = controllerutil.CreateOrUpdate(ctx, r.Client, svc, func() error {
		if err := controllerutil.SetControllerReference(gw, svc, r.Scheme); err != nil {
			return err
		}
		applyLabels(&svc.ObjectMeta, gw.Name, "control-plane")
		svc.Spec.Selector = selectorLabels(gw.Name, "control-plane")
		svc.Spec.Ports = []corev1.ServicePort{{
			Name: "status", Port: ctlStatusPort, TargetPort: intstr.FromString("status"), Protocol: corev1.ProtocolTCP,
		}, {
			Name: "fleet", Port: ctlListenPort, TargetPort: intstr.FromString("fleet"), Protocol: corev1.ProtocolTCP,
		}}
		return nil
	})
	return err
}

func (r *Reconciler) ensureDataPlanes(ctx context.Context, gw *gwv1.LLMGateway, n childNameSet) error {
	image := r.DefaultGatewaydImage
	listen := int32(8080)
	if gw.Spec.DataPlanes != nil {
		if gw.Spec.DataPlanes.Image != "" {
			image = gw.Spec.DataPlanes.Image
		}
		if gw.Spec.DataPlanes.ListenPort != nil {
			listen = *gw.Spec.DataPlanes.ListenPort
		}
	}
	// Replicas + labels come from the shared topology resolver: gatewayctl
	// is handed one --label-token per replica (same suffix scheme as
	// tokenSelectScript), so ANY replica count gets a distinct token — the
	// old 3-token cap is gone, and the CR's failure-domain labels ride the
	// tokens to the fleet, where wave plans and GatewaySets select on them.
	replicas, labels := dataPlaneTopology(gw)
	_ = labels // carried by the join tokens; pods need no label knowledge
	// gatewayd dials the control plane with tonic, which requires an explicit
	// scheme on the endpoint URI (the http:// the standalone fleet demo uses);
	// a schemeless host:port fails the HTTP/2 handshake with a transport error.
	ctlEndpoint := fmt.Sprintf("http://%s.%s.svc.cluster.local:%d", n.ctlService, gw.Namespace, ctlListenPort)

	// Data planes run as a StatefulSet, NOT a Deployment, for two reasons the
	// fleet join model requires:
	//   1. STABLE node-id across restarts. A join token binds to the FIRST
	//      node-id that burns it; only that same node-id may reconnect. A
	//      Deployment's random pod names change on reschedule, so a restarted
	//      pod would present a burned token under a NEW identity and be refused
	//      as a replay. A StatefulSet gives each pod a stable name
	//      (<name>-gatewayd-<ordinal>) that survives restart -> reconnect works.
	//   2. DISTINCT token per node. gatewayctl mints single-use tokens X, X-2,
	//      X-3 from --join-token X. The pod selects its token BY ORDINAL from
	//      its stable name, so ordinal 0->X, 1->X-2, 2->X-3. Each node burns a
	//      different token; a restart re-presents the same one as the same id.
	ss := &appsv1.StatefulSet{ObjectMeta: metav1.ObjectMeta{Name: n.dpDeploy, Namespace: gw.Namespace}}
	_, err := controllerutil.CreateOrUpdate(ctx, r.Client, ss, func() error {
		if err := controllerutil.SetControllerReference(gw, ss, r.Scheme); err != nil {
			return err
		}
		applyLabels(&ss.ObjectMeta, gw.Name, "data-plane")
		sel := selectorLabels(gw.Name, "data-plane")
		ss.Spec.Replicas = &replicas
		ss.Spec.ServiceName = n.dpService
		ss.Spec.Selector = &metav1.LabelSelector{MatchLabels: sel}
		ss.Spec.Template.ObjectMeta.Labels = sel
		// The pod derives its ordinal from its stable name, selects the matching
		// single-use token, and uses the stable name as its node-id so a restart
		// reconnects on the same identity. tokenSelectScript is POSIX sh.
		startCmd := fmt.Sprintf(
			"%s\nexec /usr/local/bin/gatewayd --control-plane %s --node-id \"$POD_NAME\" "+
				"--join-token \"$NODE_TOKEN\" --listen 0.0.0.0:%d",
			tokenSelectScript, ctlEndpoint, listen)
		ss.Spec.Template.Spec.Containers = []corev1.Container{{
			Name:    "gatewayd",
			Image:   image,
			Command: []string{"/bin/sh", "-c"},
			Args:    []string{startCmd},
			Env: []corev1.EnvVar{
				{Name: "POD_NAME", ValueFrom: &corev1.EnvVarSource{FieldRef: &corev1.ObjectFieldSelector{FieldPath: "metadata.name"}}},
				{Name: joinTokenEnvVar, ValueFrom: &corev1.EnvVarSource{SecretKeyRef: &corev1.SecretKeySelector{
					LocalObjectReference: corev1.LocalObjectReference{Name: n.secret}, Key: joinTokenKey}}},
			},
			Ports:          []corev1.ContainerPort{{Name: "proxy", ContainerPort: listen}},
			LivenessProbe:  tcpProbe("proxy", 5, 15),
			ReadinessProbe: tcpProbe("proxy", 3, 5),
			Resources:      defaultResources("50m", "64Mi", "500m", "256Mi"),
		}}
		_ = labels // failure-domain labels flow via label-token in a follow-up
		return nil
	})
	if err != nil {
		return err
	}

	// Headless Service backing the StatefulSet's stable network ids, and the
	// stable name clients dial for the data-plane proxy.
	svc := &corev1.Service{ObjectMeta: metav1.ObjectMeta{Name: n.dpService, Namespace: gw.Namespace}}
	_, err = controllerutil.CreateOrUpdate(ctx, r.Client, svc, func() error {
		if err := controllerutil.SetControllerReference(gw, svc, r.Scheme); err != nil {
			return err
		}
		applyLabels(&svc.ObjectMeta, gw.Name, "data-plane")
		svc.Spec.Selector = selectorLabels(gw.Name, "data-plane")
		svc.Spec.Ports = []corev1.ServicePort{{
			Name: "proxy", Port: listen, TargetPort: intstr.FromString("proxy"), Protocol: corev1.ProtocolTCP,
		}}
		return nil
	})
	return err
}

// --- status + observation helpers -------------------------------------------

func (r *Reconciler) deploymentReady(ctx context.Context, ns, name string) (bool, error) {
	var dep appsv1.Deployment
	if err := r.Get(ctx, types.NamespacedName{Namespace: ns, Name: name}, &dep); err != nil {
		if apierrors.IsNotFound(err) {
			return false, nil
		}
		return false, err
	}
	desired := int32(1)
	if dep.Spec.Replicas != nil {
		desired = *dep.Spec.Replicas
	}
	return dep.Status.ReadyReplicas >= desired && desired > 0, nil
}

func (r *Reconciler) statefulSetReplicas(ctx context.Context, ns, name string) (ready, desired int32, err error) {
	var ss appsv1.StatefulSet
	if e := r.Get(ctx, types.NamespacedName{Namespace: ns, Name: name}, &ss); e != nil {
		if apierrors.IsNotFound(e) {
			return 0, 0, nil
		}
		return 0, 0, e
	}
	desired = int32(1)
	if ss.Spec.Replicas != nil {
		desired = *ss.Spec.Replicas
	}
	return ss.Status.ReadyReplicas, desired, nil
}

func (r *Reconciler) fail(ctx context.Context, gw *gwv1.LLMGateway, reason, msg string) (ctrl.Result, error) {
	setCondition(&gw.Status, "Ready", metav1.ConditionFalse, reason, msg, gw.Generation)
	setCondition(&gw.Status, "Degraded", metav1.ConditionTrue, reason, msg, gw.Generation)
	gw.Status.ObservedGeneration = gw.Generation
	if err := r.Status().Update(ctx, gw); err != nil {
		return ctrl.Result{}, err
	}
	// Requeue with backoff by returning an error carrying the reason.
	return ctrl.Result{}, fmt.Errorf("%s: %s", reason, msg)
}

// SetupWithManager wires the controller to watch the CR and own its children.
func (r *Reconciler) SetupWithManager(mgr ctrl.Manager) error {
	return ctrl.NewControllerManagedBy(mgr).
		For(&gwv1.LLMGateway{}).
		Owns(&appsv1.Deployment{}).
		Owns(&appsv1.StatefulSet{}).
		Owns(&corev1.Service{}).
		Owns(&corev1.ConfigMap{}).
		Owns(&corev1.Secret{}).
		Complete(r)
}

// --- small builders ----------------------------------------------------------

func tcpProbe(port string, initialDelay, period int32) *corev1.Probe {
	return &corev1.Probe{
		ProbeHandler:        corev1.ProbeHandler{TCPSocket: &corev1.TCPSocketAction{Port: intstr.FromString(port)}},
		InitialDelaySeconds: initialDelay,
		PeriodSeconds:       period,
		FailureThreshold:    3,
	}
}

func defaultResources(cpuReq, memReq, cpuLim, memLim string) corev1.ResourceRequirements {
	return corev1.ResourceRequirements{
		Requests: corev1.ResourceList{
			corev1.ResourceCPU:    resource.MustParse(cpuReq),
			corev1.ResourceMemory: resource.MustParse(memReq),
		},
		Limits: corev1.ResourceList{
			corev1.ResourceCPU:    resource.MustParse(cpuLim),
			corev1.ResourceMemory: resource.MustParse(memLim),
		},
	}
}

// unpackScript reconstructs the '/'->'__' flattened ConfigMap keys into the
// repo directory tree gatewayctl --repo reads.
const unpackScript = `set -e
for f in /repo-src/*; do
  key=$(basename "$f")
  rel=$(echo "$key" | sed 's|__|/|g')
  dst="/repo/$rel"
  mkdir -p "$(dirname "$dst")"
  cp "$f" "$dst"
done
echo "unpacked repo:"; find /repo -type f | sort`

// --- the fleet-commit gate: gatewayctl's status surface ---------------------

// fleetStatus is the JSON gatewayctl serves on GET /status: the applied
// render identity plus every node's last-said state, verbatim.
type fleetStatus struct {
	Applied struct {
		RenderHash   string `json:"render_hash"`
		SourceCommit string `json:"source_commit"`
	} `json:"applied"`
	CommittedVersion int64 `json:"committed_version"`
	Nodes            []struct {
		NodeID       string            `json:"node_id"`
		Labels       map[string]string `json:"labels"`
		Connected    bool              `json:"connected"`
		AckedVersion *int64            `json:"acked_version"`
		AckedHash    *string           `json:"acked_hash"`
	} `json:"nodes"`
}

// queryFleetStatus reads gatewayctl's read-only status endpoint through the
// control-plane Service. A short timeout: a slow or absent answer keeps
// Ready false with a precise reason; it never blocks the reconcile loop.
func (r *Reconciler) queryFleetStatus(ctx context.Context, namespace, ctlService string) (*fleetStatus, error) {
	url := fmt.Sprintf("http://%s.%s.svc.cluster.local:%d/status", ctlService, namespace, ctlStatusPort)
	reqCtx, cancel := context.WithTimeout(ctx, 3*time.Second)
	defer cancel()
	req, err := http.NewRequestWithContext(reqCtx, http.MethodGet, url, nil)
	if err != nil {
		return nil, err
	}
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("status endpoint returned %d", resp.StatusCode)
	}
	body, err := io.ReadAll(io.LimitReader(resp.Body, 1<<20))
	if err != nil {
		return nil, err
	}
	var fs fleetStatus
	if err := json.Unmarshal(body, &fs); err != nil {
		return nil, fmt.Errorf("status endpoint returned unparseable JSON: %w", err)
	}
	return &fs, nil
}

// nodeStatuses maps the fleet's answer onto the CR's status.nodes. Healthy
// means connected AND acked exactly the currently-applied render.
func (fs *fleetStatus) nodeStatuses() []gwv1.NodeStatus {
	out := make([]gwv1.NodeStatus, 0, len(fs.Nodes))
	for _, n := range fs.Nodes {
		ns := gwv1.NodeStatus{NodeID: n.NodeID}
		if n.AckedVersion != nil {
			ns.AckedVersion = *n.AckedVersion
		}
		if n.AckedHash != nil {
			ns.AckedHash = *n.AckedHash
		}
		ns.Healthy = n.Connected && n.AckedHash != nil && *n.AckedHash == fs.Applied.RenderHash
		out = append(out, ns)
	}
	return out
}

// committed reports whether the fleet has COMMITTED the applied render:
// exactly the desired number of nodes, every one connected and acking the
// applied render_hash. The message names what is still missing.
func (fs *fleetStatus) committed(desired int32) (bool, string) {
	acked := 0
	for _, n := range fs.Nodes {
		if n.Connected && n.AckedHash != nil && *n.AckedHash == fs.Applied.RenderHash {
			acked++
		}
	}
	if int32(len(fs.Nodes)) < desired {
		return false, fmt.Sprintf("%d/%d data plane(s) joined the fleet", len(fs.Nodes), desired)
	}
	if int32(acked) < desired {
		return false, fmt.Sprintf("%d/%d data plane(s) acked the applied render %.12s",
			acked, desired, fs.Applied.RenderHash)
	}
	return true, ""
}
