package controllers

import (
	"fmt"
	"sort"
	"strings"

	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"

	gwv1 "github.com/Lyqed/smallgateway/deploy/operator/api/v1alpha1"
)

// schemeAlias is the concrete scheme type injected by main.go.
type schemeAlias = runtime.Scheme

// childNameSet holds the deterministic names of every child object of one CR.
// Names are pure functions of the CR name, so reconcile is idempotent and a
// second controller instance would target the identical objects.
type childNameSet struct {
	secret     string
	repoCM     string
	ctlDeploy  string
	ctlService string
	dpDeploy   string
	dpService  string
}

func childNames(cr string) childNameSet {
	return childNameSet{
		secret:     cr + "-join-token",
		repoCM:     cr + "-config-repo",
		ctlDeploy:  cr + "-gatewayctl",
		ctlService: cr + "-gatewayctl",
		dpDeploy:   cr + "-gatewayd",
		dpService:  cr + "-gatewayd",
	}
}

// flattenPath maps a repo-relative path to a ConfigMap key (keys cannot contain
// '/'). The init container reverses it.
func flattenPath(p string) string { return strings.ReplaceAll(p, "/", "__") }

const (
	labelName      = "app.kubernetes.io/name"
	labelInstance  = "app.kubernetes.io/instance"
	labelComponent = "app.kubernetes.io/component"
	labelManagedBy = "app.kubernetes.io/managed-by"
	labelPartOf    = "app.kubernetes.io/part-of"
)

func selectorLabels(cr, component string) map[string]string {
	return map[string]string{
		labelInstance:  cr,
		labelComponent: component,
		labelName:      "the-gateway-project",
	}
}

func applyLabels(om *metav1.ObjectMeta, cr, component string) {
	if om.Labels == nil {
		om.Labels = map[string]string{}
	}
	for k, v := range selectorLabels(cr, component) {
		om.Labels[k] = v
	}
	om.Labels[labelManagedBy] = "gateway-operator"
	om.Labels[labelPartOf] = "the-gateway-project"
}

// setCondition upserts a metav1.Condition, preserving lastTransitionTime when
// the status is unchanged (standard condition semantics).
func setCondition(st *gwv1.LLMGatewayStatus, condType string, status metav1.ConditionStatus, reason, message string, gen int64) {
	now := metav1.Now()
	for i := range st.Conditions {
		if st.Conditions[i].Type == condType {
			if st.Conditions[i].Status != status {
				st.Conditions[i].LastTransitionTime = now
			}
			st.Conditions[i].Status = status
			st.Conditions[i].Reason = reason
			st.Conditions[i].Message = message
			st.Conditions[i].ObservedGeneration = gen
			return
		}
	}
	st.Conditions = append(st.Conditions, metav1.Condition{
		Type:               condType,
		Status:             status,
		Reason:             reason,
		Message:            message,
		LastTransitionTime: now,
		ObservedGeneration: gen,
	})
}

// dataPlaneTopology resolves the CR's data-plane replica count and
// failure-domain labels (defaults: 1 replica, no labels).
func dataPlaneTopology(gw *gwv1.LLMGateway) (int32, map[string]string) {
	replicas := int32(1)
	var labels map[string]string
	if gw.Spec.DataPlanes != nil {
		if gw.Spec.DataPlanes.Replicas != nil {
			replicas = *gw.Spec.DataPlanes.Replicas
		}
		labels = gw.Spec.DataPlanes.Labels
	}
	if replicas < 1 {
		replicas = 1
	}
	return replicas, labels
}

// labelTokenSpec renders the CR's failure-domain labels into gatewayctl's
// --label-token label list ("k=v,k2=v2"; empty for no labels). Label keys
// and values must stay inside a conservative charset: they ride a shell
// command line and the token grammar (':' separates labels from secret,
// ',' separates pairs).
func labelTokenSpec(labels map[string]string) (string, error) {
	if len(labels) == 0 {
		return "", nil
	}
	keys := make([]string, 0, len(labels))
	for k := range labels {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	parts := make([]string, 0, len(keys))
	for _, k := range keys {
		v := labels[k]
		for _, s := range []string{k, v} {
			if s == "" || strings.ContainsAny(s, ",:'\"\\$` \t\n") {
				return "", fmt.Errorf("label %q=%q contains characters the token grammar does not accept", k, v)
			}
		}
		parts = append(parts, k+"="+v)
	}
	return strings.Join(parts, ","), nil
}

// ctlCommand builds the gatewayctl invocation: the fleet listener, the
// status surface, and ONE --label-token per data-plane replica carrying the
// CR's failure-domain labels — the per-ordinal secrets derive from the one
// Secret env var with the same suffix scheme tokenSelectScript uses, so the
// secret itself never appears in the pod spec. With label tokens present,
// gatewayctl mints NO unlabeled base tokens and no dev default.
func ctlCommand(gw *gwv1.LLMGateway) string {
	replicas, labels := dataPlaneTopology(gw)
	labelSpec, err := labelTokenSpec(labels)
	if err != nil {
		// Unreachable: ensureCtl validates before building. Defensive default.
		labelSpec = ""
	}
	var b strings.Builder
	fmt.Fprintf(&b, "exec /usr/local/bin/gatewayctl --repo /repo --listen 0.0.0.0:%d --status-listen 0.0.0.0:%d",
		ctlListenPort, ctlStatusPort)
	for i := int32(0); i < replicas; i++ {
		suffix := ""
		if i > 0 {
			suffix = fmt.Sprintf("-%d", i+1)
		}
		fmt.Fprintf(&b, " --label-token '%s:'\"$%s\"%s", labelSpec, joinTokenEnvVar, suffix)
	}
	return b.String()
}
