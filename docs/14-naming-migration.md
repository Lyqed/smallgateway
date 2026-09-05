# Naming changes

The project uses smallgateway throughout its deployment examples and API names.
The Helm chart version is now 0.2.0 to mark the API-group change.

| Previous name | Current name |
| --- | --- |
| `gateway.opensourcegateway.com/v1alpha1` | `gateway.smallgateway.vercel.app/v1alpha1` |
| `spec.rejections.missingAttribution` | `spec.rejections.defaultResponse` |
| `rejections.missing_attribution` | `rejections.default_response` |

The response was renamed because it also acts as a fallback for several refusal
reasons. A dedicated response, such as `cap_exceeded`, still takes precedence
where configured. Status codes, bodies, and streaming behavior are unchanged.

Existing gateway YAML can still use `missing_attribution`. It is a parser alias
for `default_response`, including scope overrides. Do not set both names.
The operator and examples now generate only the new key.

## Kubernetes installations

Changing an API group creates a different Kubernetes resource. Applying the new
CRD does not migrate existing objects. Helm does not automatically upgrade CRDs
from a chart's `crds/` directory.

Before upgrading an existing installation:

1. Export and keep a backup of the old custom resources and operator settings.
2. Plan a maintenance window. Stop the old controller before enabling the new
   one; their leader-election lock names differ.
3. Install the new CRD and update manifests to the new API version and
   `defaultResponse` field. Use the matching operator and RBAC.
4. Check generated workloads and owner references carefully. The old and new
   resources have different UIDs even if their names match.
5. Verify reconciliation, request handling, and configured rejection responses.
   Keep the old CRD and backups until migration and rollback have been reviewed.

Do not delete the old CRD as a shortcut: Kubernetes deletes its custom resources,
which can also trigger deletion of resources they own.

These repository changes do not migrate a running cluster.
