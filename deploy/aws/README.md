# AWS IAM for shapes 2 and 3 — ready to apply

The three policy documents that back the Bedrock shapes in
[docs/10-getting-started.md](../../docs/10-getting-started.md): the
gateway's base identity, the target role that REFUSES untagged
assumption, and a permissions boundary scoped to the fleet's models.
Edit the placeholders; the shape is the point.

| File | Attach to | What it does |
|---|---|---|
| `base-role-trust.json` | the base role (`gatewayd-base`) | lets ONLY the gateway's platform identity (its OIDC token) assume the base — hop 1 of the chain |
| `target-role-trust.json` | each target role | lets ONLY the base role assume it, and ONLY with the session tags present — untagged assumption fails at AWS itself |
| `target-role-permissions.json` | each target role | scopes what an assumed session may invoke, down to individual foundation models |

What to replace:

- `111122223333` — your account id(s). The target role may live in a
  DIFFERENT account than the base (the cross-account layout in the
  estates section); only these trust policies decide.
- `OIDC_PROVIDER_PATH` — your cluster's OIDC provider path, e.g.
  `oidc.eks.eu-central-1.amazonaws.com/id/EXAMPLED539D4633E53DE1B71EXAMPLE`,
  and the `sub` condition's namespace/service-account to match your
  deployment.
- `cost_center` / `workload` — the session-tag keys are the FLEET's own
  attribution keys, exactly as configured under `sts.tags`. These two
  match the getting-started example; yours may differ.
- `anthropic.claude*` — the model resources this role may invoke.

Two variants worth knowing:

**Close the value set at AWS too.** `StringLike: "*"` requires the tag
to be PRESENT; to also pin its values (the IAM twin of the gateway's
`allow:` list), swap in:

```json
"StringEquals": {
  "aws:RequestTag/cost_center": ["research", "radiology", "pharmacy"]
}
```

**Per-team roles (shape 3).** Apply `target-role-trust.json` to every
role the `role_arn` template can name (`bedrock-research`,
`bedrock-radiology`, ...), each with its own permissions file — that is
the IAM-level separation the templated chain exists for.

Why the trust policy matters more than it looks: with
`sts:TagSession` + `aws:RequestTag` conditions in place, the
discipline the gateway enforces is also MANDATORY one layer down —
nothing, including a misconfigured or bypassed gateway, can assume the
target role without carrying the attribution. The tags these policies
require are the same ones that reach CloudTrail on every assumption
and, activated, the bill.
