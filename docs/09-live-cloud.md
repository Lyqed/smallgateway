# Live-cloud confirmation runbook

Every mechanism below is verified end-to-end against mocks that enforce the
live wire contracts (SigV4 recompute + body-hash checks, chained-call
signature verification, WIF URN/audience/lifetime shapes, bearer
enforcement). This runbook is the remaining step: point the same config at
real AWS and GCP once, watch the same log lines, and see the attribution
land on the real invoice. No code changes are expected; that is the point
of the strict mocks.

Nothing here contains secrets. Account IDs, project numbers, and role
names are placeholders.

## AWS: the two-hop role chain onto the CUR

### Cloud-side setup (once)

1. **OIDC provider** in IAM for the platform issuing the gateway's
   web-identity token (EKS: the cluster's OIDC issuer, usually already
   registered).
2. **Base role** `gatewayd-base`: trust policy allows
   `sts:AssumeRoleWithWebIdentity` from that OIDC provider (bound to the
   gateway's service account subject). Identity policy: `sts:AssumeRole` +
   `sts:TagSession` on the target role ARNs (a wildcard like
   `arn:aws:iam::<acct>:role/bedrock-*` keeps it one statement).
3. **Target roles** (one per operator-chosen value, e.g.
   `bedrock-research`, `bedrock-radiology`): trust policy allows
   `sts:AssumeRole` + `sts:TagSession` from `gatewayd-base`;
   identity policy grants `bedrock:InvokeModel*`. MaxSessionDuration can
   stay 3600 (role chaining caps there anyway; the gateway validates it).
4. **Cost allocation tags**: in the Billing console, activate the session
   tag keys the fleet uses (whatever the operator named them) as
   cost-allocation tags. CUR/Cost Explorer columns appear within a day.

### Gateway config (the shape the conformance suite runs)

```yaml
providers:
  bedrock-main:
    kind: bedrock
    upstream: { host: bedrock-runtime.us-east-1.amazonaws.com, port: 443, tls: true }
    sts:
      endpoint: { host: sts.us-east-1.amazonaws.com, port: 443, tls: true }
      region: us-east-1
      role_arn: arn:aws:iam::<acct>:role/bedrock-{{cost_center}}
      session_name: '{{cost_center}}-{{workload}}'
      base:
        web_identity_token: { file: /var/run/secrets/tokens/gateway-token }
        role_arn: arn:aws:iam::<acct>:role/gatewayd-base
        sts_region: us-east-1
      tags:
        - { key: cost_center, from_attribution: cost_center }
        - { key: workload, from_attribution: workload }
```

NOTE: `sts.endpoint`/`base` currently speak HTTP/1.1 to the given
host:port; live STS is TLS on 443. The internal STS client's TLS support
is the one seam to confirm first (the mock pair runs plaintext). If the
plain-TCP client meets live TLS, front STS via an egress proxy that
terminates TLS, or wire rustls into `http_post` — a contained change to
one function.

### What to watch

- `[gb7] base hop: role=...` then `[gb7] AssumeRole ok: ... chained=true`
  in gatewayd logs: the chain worked against live STS.
- A Bedrock 200: the signed payload hash was accepted.
- CloudTrail: `AssumeRole` events with your RoleSessionName values;
  Bedrock `InvokeModel` events attributed to the per-value role sessions.
- After ~24h: Cost Explorer, group by your activated tag key. The line
  items ARE the proof; nothing of ours sits between the tags and the bill.

## GCP: WIF onto the billing export

### Cloud-side setup (once)

1. **Workload Identity Pool + provider** for the platform's OIDC issuer
   (`gcloud iam workload-identity-pools create` /
   `...providers create-oidc`), attribute-mapped to the gateway's subject.
2. **Service account** `gateway@<project>.iam.gserviceaccount.com` with
   `roles/aiplatform.user`; grant the pool identity
   `roles/iam.workloadIdentityUser` on it, and the pool's federated
   identity `roles/iam.serviceAccountTokenCreator` (for
   generateAccessToken).
3. **Billing export to BigQuery** enabled (labels arrive in the export).

### Gateway config

```yaml
providers:
  vertex-main:
    kind: vertex
    upstream: { host: aiplatform.googleapis.com, port: 443, tls: true }
    locations: [eu, europe-west3, europe-west4]
    auth:
      web_identity_token: { file: /var/run/secrets/tokens/gateway-token }
      wif:
        project_number: "<project-number>"
        pool_id: gw-pool
        provider_id: gw-provider
      service_account_email: gateway@<project>.iam.gserviceaccount.com
      sts_endpoint: { host: sts.googleapis.com, port: 443, tls: true }
      iam_endpoint: { host: iamcredentials.googleapis.com, port: 443, tls: true }
```

Same TLS seam note as AWS applies to `sts_endpoint`/`iam_endpoint`.

### What to watch

- `[gb8] SA token minted: sa=...` then `[gb8-auth ...] cache=hit` on the
  next request: the WIF chain worked and the cache holds.
- Vertex 200s through a regional location: the derived host + SNI held.
- The billing export:
  `SELECT l.value, ROUND(SUM(cost),2) FROM billing_export, UNNEST(labels) l
   WHERE l.key = '<your-key>' GROUP BY 1` — the operator's labels on real
  spend.

## The honest checklist

- [ ] STS/Google-endpoint TLS confirmed (the one known seam; everything
      else ran against contract-enforcing mocks).
- [ ] AWS: chained AssumeRole 200 + CloudTrail session names + tagged CUR
      line items.
- [ ] GCP: SA token minted + cached + labeled line items in the export.
- [ ] Rejection paths unchanged live (GB-1 428 body, location-gate 404
      body, over-cap 429/cut) — the operator's words, verbatim.
