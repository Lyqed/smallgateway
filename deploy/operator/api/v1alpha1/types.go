// Package v1alpha1 is the Go type surface for the LLMGateway CRD
// (gateway.thegatewayproject.io/v1alpha1). The fields mirror the CRD's
// openAPIV3Schema 1:1; they map onto the existing gateway-core config model,
// never a parallel one. See deploy/crds/llmgateway.yaml for the authoritative
// schema and deploy/README.md for the field reference.
package v1alpha1

import (
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/runtime/schema"
	"sigs.k8s.io/controller-runtime/pkg/scheme"
)

const (
	GroupName = "gateway.thegatewayproject.io"
	Version   = "v1alpha1"
)

var (
	// GroupVersion is the group/version this package registers.
	GroupVersion = schema.GroupVersion{Group: GroupName, Version: Version}
	// SchemeBuilder registers the types with a runtime scheme.
	SchemeBuilder = &scheme.Builder{GroupVersion: GroupVersion}
	// AddToScheme adds this group's types to a scheme.
	AddToScheme = SchemeBuilder.AddToScheme
)

func init() {
	SchemeBuilder.Register(&LLMGateway{}, &LLMGatewayList{})
}

// LLMGateway is the k8s-native config surface for the gateway.
type LLMGateway struct {
	metav1.TypeMeta   `json:",inline"`
	metav1.ObjectMeta `json:"metadata,omitempty"`

	Spec   LLMGatewaySpec   `json:"spec"`
	Status LLMGatewayStatus `json:"status,omitempty"`
}

// LLMGatewayList is the list wrapper.
type LLMGatewayList struct {
	metav1.TypeMeta `json:",inline"`
	metav1.ListMeta `json:"metadata,omitempty"`
	Items           []LLMGateway `json:"items"`
}

// LLMGatewaySpec is the desired gateway. Every field renders into a fragment
// of the existing gatewayctl config-repo layout.
type LLMGatewaySpec struct {
	Providers    map[string]Provider `json:"providers"`
	Fleet        *Scope              `json:"fleet,omitempty"`
	Routes       []Route             `json:"routes"`
	Projects     map[string]Scope    `json:"projects,omitempty"`
	Rejections   *Rejections         `json:"rejections,omitempty"`
	SpendCaps    *SpendCaps          `json:"spendCaps,omitempty"`
	// GB-2 JWT auth is deliberately NOT in v1alpha1 — it is project-deferred
	// (see README "Follow-ups"). No auth field is exposed so a CR cannot claim
	// JWT enforcement the operator does not implement.
	ControlPlane *ControlPlaneSpec `json:"controlPlane,omitempty"`
	DataPlanes   *DataPlanesSpec     `json:"dataPlanes,omitempty"`
	// GatewayClassName, if set, claims a standard gateway.networking.k8s.io
	// GatewayClass (the standards path adapter). See README "Gateway API".
	GatewayClassName string `json:"gatewayClassName,omitempty"`
}

// Provider is one named upstream.
type Provider struct {
	Kind     string   `json:"kind"`
	Upstream Upstream `json:"upstream"`
}

// Upstream is a provider's backend address.
type Upstream struct {
	Host string `json:"host"`
	Port int    `json:"port"`
	TLS  bool   `json:"tls,omitempty"`
	SNI  string `json:"sni,omitempty"`
}

// Scope is an attribution chain (fleet or project scope).
type Scope struct {
	Attribution *Attribution `json:"attribution,omitempty"`
}

// Attribution is a scope's GB-1 required keys and GB-3 pins.
type Attribution struct {
	RequiredKeys []string `json:"requiredKeys,omitempty"`
	// Headers names the EXACT caller header per attribution key. There is
	// no default header name: a required key with no gateway origin and no
	// entry here is refused by the gateway at config load.
	Headers map[string]string `json:"headers,omitempty"`
	Pinned  map[string]string `json:"pinned,omitempty"`
	// Model allow-list for this scope: exact names or a trailing-* family
	// (claude-3*). A lower scope's list REPLACES a higher one's.
	Models []string `json:"models,omitempty"`
}

// Route is one path-prefix route.
type Route struct {
	Name        string       `json:"name"`
	Prefix      string       `json:"prefix"`
	Provider    string       `json:"provider"`
	Project     string       `json:"project,omitempty"`
	Match       string       `json:"match,omitempty"`
	Attribution *Attribution `json:"attribution,omitempty"`
}

// Rejections holds GB-4 operator-owned rejection bodies.
type Rejections struct {
	MissingAttribution *RejectionTemplate `json:"missingAttribution,omitempty"`
	UnknownRoute       *RejectionTemplate `json:"unknownRoute,omitempty"`
}

// RejectionTemplate is one rejection body.
type RejectionTemplate struct {
	Status      int                 `json:"status"`
	ContentType string              `json:"contentType"`
	Body        string              `json:"body"`
	Streaming   *StreamingRejection `json:"streaming,omitempty"`
}

// StreamingRejection is the terminal event for a cut stream.
type StreamingRejection struct {
	Event string `json:"event,omitempty"`
	Data  string `json:"data"`
}

// SpendCaps holds GB-5 fleet spend caps.
type SpendCaps struct {
	Caps []SpendCap `json:"caps,omitempty"`
}

// SpendCap is one attributed-spend ceiling.
type SpendCap struct {
	Key   string `json:"key"`
	Value string `json:"value"`
	// The ceiling in TOKENS. The gateway meters tokens (its authoritative
	// quantity); dollars are the cloud invoice's meter, not ours.
	LimitTokens int64  `json:"limitTokens"`
	Window      string `json:"window,omitempty"`
	// GB-6 alert threshold in percent (1-100); alert at N, enforce at 100.
	AlertAt int32 `json:"alertAt,omitempty"`
}

// ControlPlaneSpec is the gatewayctl topology.
type ControlPlaneSpec struct {
	Image     string                `json:"image,omitempty"`
	Replicas  *int32                `json:"replicas,omitempty"`
	Resources *runtime.RawExtension `json:"resources,omitempty"`
}

// DataPlanesSpec is the gatewayd topology.
type DataPlanesSpec struct {
	Image      string                `json:"image,omitempty"`
	Replicas   *int32                `json:"replicas,omitempty"`
	ListenPort *int32                `json:"listenPort,omitempty"`
	Labels     map[string]string     `json:"labels,omitempty"`
	Resources  *runtime.RawExtension `json:"resources,omitempty"`
}

// LLMGatewayStatus reflects the real reconciled state.
type LLMGatewayStatus struct {
	ObservedGeneration int64              `json:"observedGeneration,omitempty"`
	RenderedConfigHash string             `json:"renderedConfigHash,omitempty"`
	DataPlanes         string             `json:"dataPlanes,omitempty"`
	ControlPlaneReady  bool               `json:"controlPlaneReady,omitempty"`
	Conditions         []metav1.Condition `json:"conditions,omitempty"`
	Nodes              []NodeStatus       `json:"nodes,omitempty"`
}

// NodeStatus is per-node ack state (populated when an ack-query path exists).
type NodeStatus struct {
	NodeID       string `json:"nodeId,omitempty"`
	AckedVersion int64  `json:"ackedVersion,omitempty"`
	AckedHash    string `json:"ackedHash,omitempty"`
	Healthy      bool   `json:"healthy,omitempty"`
}

// DeepCopyObject implementations. Hand-written (no controller-gen dependency)
// so the operator builds with only controller-runtime + client-go.

func (in *LLMGateway) DeepCopyObject() runtime.Object {
	if in == nil {
		return nil
	}
	out := new(LLMGateway)
	in.DeepCopyInto(out)
	return out
}

func (in *LLMGateway) DeepCopyInto(out *LLMGateway) {
	*out = *in
	out.TypeMeta = in.TypeMeta
	in.ObjectMeta.DeepCopyInto(&out.ObjectMeta)
	// Spec/Status contain maps and slices; a JSON round-trip is the simplest
	// correct deep copy for a config object of this size and avoids a large
	// hand-written generated file. Reconcile never mutates the cached object,
	// so this path is only exercised by the informer's copy-out.
	deepCopyViaJSON(&in.Spec, &out.Spec)
	deepCopyViaJSON(&in.Status, &out.Status)
}

func (in *LLMGatewayList) DeepCopyObject() runtime.Object {
	if in == nil {
		return nil
	}
	out := new(LLMGatewayList)
	out.TypeMeta = in.TypeMeta
	in.ListMeta.DeepCopyInto(&out.ListMeta)
	if in.Items != nil {
		out.Items = make([]LLMGateway, len(in.Items))
		for i := range in.Items {
			in.Items[i].DeepCopyInto(&out.Items[i])
		}
	}
	return out
}
