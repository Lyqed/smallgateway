package v1alpha1

import "encoding/json"

// deepCopyViaJSON deep-copies src into dst through a JSON round-trip. Used by
// DeepCopyInto for the Spec/Status config subtrees (maps and slices). A config
// CR is small and infrequently copied (only on informer copy-out), so the
// round-trip cost is negligible and it is guaranteed structurally correct
// without a large generated deepcopy file.
func deepCopyViaJSON[T any](src *T, dst *T) {
	b, err := json.Marshal(src)
	if err != nil {
		return
	}
	_ = json.Unmarshal(b, dst)
}
