# The demo estate — a real bill you can show

*A companion to [09-live-cloud.md](09-live-cloud.md), scoped down. That runbook
targets a production two-hop chain from an EKS OIDC issuer. This one stands up
the smallest REAL Bedrock estate that produces a genuine, shareable AWS Cost
and Usage Report attributed by team — in a single personal account, no
Kubernetes, no employer data. The output is the reconciliation exhibit with
true numbers.*

The whole point is that nothing in the exhibit is invented. The token columns
come from the gateway's own meter; the USD column is the line AWS billed; they
match because the value the gateway pinned as a session tag is the value AWS
allocated the cost to. To show that, the bill has to be real. Here is how to
make one cheaply and safely.

## What you are building

Three "teams" in one account you own, each a role the gateway assumes with a
per-request session tag, all invoking Bedrock, all landing on ONE CUR sliced
by `cost_center`.

```
gateway (local creds)
  │  AssumeRole + TagSession(cost_center=<team>)
  ├─→ role bedrock-radiology   ─┐
  ├─→ role bedrock-pharmacy     ├─→ Bedrock InvokeModel ─→ CUR line, tagged
  └─→ role bedrock-research    ─┘
```

No OIDC provider, no EKS, no web-identity token. The gateway authenticates to
STS with a plain IAM user's long-lived keys (fine for a throwaway demo
account; NEVER the shape for production — production is doc 09's federated
chain). One base identity, three target roles, session tags carrying the team.

## Cost and time, honestly

- **Spend:** a few dollars. Haiku is ~\$0.25 / \$1.25 per million in/out
  tokens; a few thousand short completions across three teams is single-digit
  dollars. Use Haiku for volume and one Sonnet team for a realistic spread.
- **CUR latency:** this is the long pole. Cost-allocation tags activate within
  ~24h; CUR 2.0 rows for tagged spend appear **within 24 hours of activation and
  populate daily thereafter**. Plan on **running traffic, then waiting a day or
  two** before the tagged columns are dense enough to screenshot. Run traffic
  across 2-3 days so the billing period has real shape.

## Step 1 — the identities (once)

In account `961068435493` (or a fresh dedicated demo account — cleaner):

1. **One IAM user** `gatewayd-demo` with programmatic access only. Identity
   policy: `sts:AssumeRole` + `sts:TagSession` on the three target role ARNs.
   Keep the keys in the gateway host's environment, nowhere else.

2. **Three target roles** — `bedrock-radiology`, `bedrock-pharmacy`,
   `bedrock-research`. Each:
   - **Trust policy:** allow `sts:AssumeRole` **and** `sts:TagSession` from
     the `gatewayd-demo` user's ARN. The `TagSession` grant is what lets the
     session carry `cost_center`; without it the tag is silently dropped and
     the CUR never sees it.
   - **Identity policy:** `bedrock:InvokeModel` and
     `bedrock:InvokeModelWithResponseStream` on the model ARNs you'll call.

   Trust policy shape (per role):
   ```json
   {
     "Version": "2012-10-17",
     "Statement": [{
       "Effect": "Allow",
       "Principal": { "AWS": "arn:aws:iam::961068435493:user/gatewayd-demo" },
       "Action": ["sts:AssumeRole", "sts:TagSession"]
     }]
   }
   ```

3. **Enable Bedrock model access** for the models you'll use (Bedrock console →
   Model access → request Claude Haiku + Sonnet; approval is usually instant
   for Anthropic models in `us-east-1`).

## Step 2 — activate the cost-allocation tag (the slow part, do it FIRST)

Billing console → **Cost allocation tags** → find `cost_center` under
**user-defined tags** → **Activate**. It only appears there *after* at least
one tagged resource/session exists, so:

- Run a handful of tagged InvokeModel calls first (step 4), wait a few hours
  for the tag key to surface, then activate it.
- Activation is retroactive to the start of the current month for CUR 2.0, but
  the column only populates going forward — so **activate early**, then let
  traffic accumulate.

Also enable **CUR 2.0** (Billing → Data Exports → Create → CUR 2.0) to S3 with
**resource IDs** and, crucially, **split cost allocation data** off — you want
the `resource_tags` / cost-allocation-tag columns and the
`line_item_usage_account_id`. Athena integration optional but makes the slice a
one-query job (step 5).

## Step 3 — the gateway config (single-account shape)

This is the simplified sibling of doc 09's config: static base creds instead of
a web-identity token, no `base` federation hop.

```yaml
providers:
  bedrock-demo:
    kind: bedrock
    upstream: { host: bedrock-runtime.us-east-1.amazonaws.com, port: 443, tls: true }
    sts:
      endpoint: { host: sts.us-east-1.amazonaws.com, port: 443, tls: true }
      region: us-east-1
      role_arn: arn:aws:iam::961068435493:role/bedrock-{{cost_center}}
      session_name: '{{cost_center}}-{{workload}}'
      # single-account demo: the gateway's own IAM user keys, from env.
      # (production replaces this whole block with the web-identity base hop.)
      static_credentials:
        access_key_id_env: GATEWAYD_DEMO_AWS_AK
        secret_access_key_env: GATEWAYD_DEMO_AWS_SK
      tags:
        - { key: cost_center, from_attribution: cost_center }
        - { key: workload, from_attribution: workload }

fleet:
  attribution:
    required_keys: [cost_center, workload]
    pinned: { env: demo }
    # the demo's three teams; a value not in this set is refused at the door.
    # (operator-pinned per key, so callers cannot forge a cost_center)
```

> **Config seam to confirm:** the `sts.static_credentials` env-var shape above
> is the single-account convenience form. If the current binary only wires the
> federated `base` hop, this is the one small addition the demo needs — a
> static-creds source for the STS client, parallel to the web-identity file.
> Check `crates/gateway-core/src/aws.rs` / the sts config struct before
> assuming it exists; if not, it is a contained add (a `Credentials` source
> that reads two env vars instead of exchanging a token).

> **The TLS seam (from doc 09, line 56):** live STS and Bedrock are TLS on 443;
> the internal STS client speaks plaintext HTTP/1.1 against the mock. For the
> demo, front STS+Bedrock through a local egress proxy that terminates TLS
> (e.g. a tiny `stunnel`/`ghostunnel` on `127.0.0.1`, pointed at the config's
> `endpoint`/`upstream`), OR wire rustls into `http_post`. Confirm this FIRST —
> it is the most likely thing to block first contact with real AWS.

## Step 4 — generate diverse, realistic traffic

The exhibit is more convincing with SHAPE: different teams, different models,
different volumes, a believable spread of input/output ratios. A small driver
script, three teams, run over 2-3 days:

- **radiology** — Sonnet, moderate volume, long inputs (imaging-report
  summaries): high input tokens.
- **pharmacy** — Sonnet, lower volume, balanced.
- **research** — Opus (or Sonnet), low volume, expensive per call.
- **claims-ops** — Haiku, high volume, short calls: the cheap-but-busy team.

Each request sends the operator-named attribution headers for its team; the
gateway pins `cost_center`, assumes the matching role with the session tag, and
calls Bedrock. Watch the gateway logs:

- `[attr /bedrock-demo] cost_center=radiology(...) workload=triage cfg=v1`
- `[gb7] AssumeRole ok: ... session_tags=[cost_center=radiology,...]`
- a Bedrock `200`, then the `[meter ...]` line with authoritative tokens.

Keep the gateway's `[meter]` lines (or the OTLP span export) — that is the
LEFT half of the exhibit, the gateway ledger. The CUR is the right half.

## Step 5 — pull the real numbers (after CUR populates)

Give it 24-48h after activation + traffic. Then, via Athena on the CUR 2.0
export (or Cost Explorer grouped by the `cost_center` tag):

```sql
SELECT
  resource_tags['user_cost_center']      AS cost_center,
  COUNT(*)                                AS line_items,
  ROUND(SUM(line_item_unblended_cost), 2) AS usd
FROM cur2
WHERE line_item_product_code = 'AmazonBedrock'
  AND bill_billing_period_start_date = DATE '2026-08-01'
GROUP BY 1
ORDER BY usd DESC;
```

The `usd` per `cost_center` is the exhibit's right column — the line AWS
billed. Join it against the gateway's own per-`cost_center` token totals (from
the `[meter]` lines / span export) on the same key. They reconcile because the
tag the gateway pinned is the tag AWS allocated to. Screenshot the CUR/Cost
Explorer view too — the "here is the actual bill" artifact is worth as much as
the reconciliation table.

## Step 6 — regenerate the exhibit with true numbers

Swap the illustrative figures in the exhibit for these. Drop the "illustrative"
badge and disclosure line. Nothing about the shape changes — that was the point
of building it from the real CUR 2.0 columns to begin with.

## The checklist

- [ ] Dedicated/clean account, IAM user `gatewayd-demo`, three tagged-assumable roles.
- [ ] Bedrock model access approved (Haiku + Sonnet at least).
- [ ] TLS seam handled (egress proxy or rustls) — confirmed with one live AssumeRole 200.
- [ ] `static_credentials` config source present (or added) — one live `[gb7] AssumeRole ok`.
- [ ] `cost_center` activated as a cost-allocation tag; CUR 2.0 export live to S3.
- [ ] 2-3 days of diverse traffic across the teams; `[meter]` lines / spans retained.
- [ ] CUR populated; Athena/Cost Explorer slice by `cost_center` matches the ledger.
- [ ] Exhibit regenerated with real numbers; illustrative disclosure removed.
- [ ] Screenshot of the raw CUR/Cost Explorer view kept as the corroborating artifact.

## What this proves (and what it does not)

It proves the mechanism end to end on real infrastructure: operator-pinned
attribution reaching AWS's own bill, per team, caller-unforgeable, reconciled
to the gateway's ledger. It does NOT prove scale or multi-account topology —
that is doc 09's production shape, and the honest framing for the exhibit is
"a small real estate," never "a customer deployment." The number is real; the
size is a demo. That distinction is the same integrity rule the tracker runs
on: show exactly what was verified, no more.
