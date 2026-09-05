package render

import (
	"bytes"
	"os"
	"testing"

	gwv1 "github.com/Lyqed/smallgateway/deploy/operator/api/v1alpha1"
	"sigs.k8s.io/yaml"
)

func TestCRDSchemaMatchesOperatorNames(t *testing.T) {
	standalone, err := os.ReadFile("../../../crds/llmgateway.yaml")
	if err != nil {
		t.Fatal(err)
	}
	chart, err := os.ReadFile("../../../charts/gateway-operator/crds/llmgateway.yaml")
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(standalone, chart) {
		t.Fatal("standalone and chart CRDs differ")
	}
	var crd map[string]any
	if err := yaml.UnmarshalStrict(standalone, &crd); err != nil {
		t.Fatal(err)
	}
	spec := crd["spec"].(map[string]any)
	if spec["group"] != gwv1.GroupName {
		t.Fatalf("CRD group %v differs from operator group %s", spec["group"], gwv1.GroupName)
	}
	version := spec["versions"].([]any)[0].(map[string]any)
	if version["name"] != gwv1.GroupVersion.Version {
		t.Fatalf("CRD version %v differs from operator", version["name"])
	}
	schema := version["schema"].(map[string]any)["openAPIV3Schema"].(map[string]any)
	props := schema["properties"].(map[string]any)["spec"].(map[string]any)["properties"].(map[string]any)
	rejections := props["rejections"].(map[string]any)["properties"].(map[string]any)
	if _, ok := rejections["defaultResponse"]; !ok {
		t.Fatal("CRD would prune the operator's defaultResponse field")
	}
}

func TestDefaultResponseFromResourceToConfig(t *testing.T) {
	var gateway gwv1.LLMGateway
	err := yaml.UnmarshalStrict([]byte(`
apiVersion: gateway.smallgateway.vercel.app/v1alpha1
kind: LLMGateway
metadata:
  name: example
spec:
  rejections:
    defaultResponse:
      status: 428
      contentType: application/json
      body: '{"error":"request refused"}'
      streaming:
        event: error
        data: '{"error":"stream refused"}'
`), &gateway)
	if err != nil {
		t.Fatal(err)
	}
	if gateway.APIVersion != gwv1.GroupVersion.String() {
		t.Fatalf("resource API %q differs from operator API %q", gateway.APIVersion, gwv1.GroupVersion)
	}
	result, err := Render(&gateway.Spec)
	if err != nil {
		t.Fatal(err)
	}
	for _, fragment := range result.Fragments {
		if fragment.Path != "rejections.yaml" {
			continue
		}
		// Rendered config uses snake_case, so inspect the YAML keys directly.
		var config map[string]any
		if err := yaml.UnmarshalStrict(fragment.Bytes, &config); err != nil {
			t.Fatal(err)
		}
		response, ok := config["default_response"].(map[string]any)
		if !ok {
			t.Fatalf("missing default_response in %s", fragment.Bytes)
		}
		if response["status"] != float64(428) || response["body"] != gateway.Spec.Rejections.DefaultResponse.Body {
			t.Fatalf("operator changed the configured response: %v", response)
		}
		if response["content_type"] != "application/json" {
			t.Fatalf("wrong content type: %v", response)
		}
		stream := response["streaming"].(map[string]any)
		if stream["event"] != "error" || stream["data"] != gateway.Spec.Rejections.DefaultResponse.Streaming.Data {
			t.Fatalf("operator changed the stream response: %v", stream)
		}
		return
	}
	t.Fatal("operator did not render rejections.yaml")
}
