package controllers

import (
	"strings"

	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"

	gwv1 "github.com/thegatewayproject/gateway-operator/api/v1alpha1"
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
