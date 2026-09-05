# Nebius and CoreWeave

Documentation checked 5 September 2026. Provider support below describes particular services, not everything sold by either company.

## What is available here

Two configurations use the existing `kind: openai` adapter:

- [Nebius Token Factory](../deploy/examples/nebius.yaml), using the public inference endpoint.
- [CoreWeave Dedicated Inference](../deploy/examples/coreweave.yaml), using an endpoint from your own gateway with body-based model routing.

These exercise chat-completion forwarding, provider bearer authentication, operator-pinned attribution, and token metering from compatible JSON or SSE responses. Local tests verify the recipes against mocks. No live request or invoice has been verified for these providers. Other API families need their own checks before we claim support.

`openai` selects a wire format. It does not select a billing model or mean the request goes to OpenAI. The recipe's provider name identifies the destination in configuration; its pinned `billing_service` value identifies it in attribution.

## Where the dollars come from

| Service | Documented billing surface | Limit of the evidence |
| --- | --- | --- |
| Nebius Token Factory public inference | Billing and consumption views support project, service, and resource breakdowns. | No documented arbitrary per-request team/app label path into the invoice was found in the reviewed billing and inference documentation. |
| Nebius Token Factory dedicated endpoints | Charges depend on active replicas and contract terms. | Shared endpoint costs need an allocation rule. A token count does not identify an individual billed charge. |
| Nebius AI Cloud infrastructure | Hourly CSV exports include invoice-related charges, adjustments, commitments, and project metadata. | Infrastructure export support does not establish that Token Factory request labels appear there. |
| CoreWeave Inference | Compute is billed through reserved nodes or GPU-hours measured at deployment level. | Splitting a shared deployment's bill between applications is an allocation. There is no per-token price to infer from the API's compatibility. |
| Microsoft Foundry | Cost Management exposes charges; current documentation also describes project attribution in preview for models sold by Azure. | That preview excludes models served through Azure Marketplace. It does not establish arbitrary caller-defined dimensions on every request. |

Sources: [Token Factory consumption](https://docs.tokenfactory.nebius.com/other-capabilities/billing-new), [dedicated endpoint billing](https://docs.tokenfactory.nebius.com/ai-models-inference/dedicated-endpoints/billing-policy), [Nebius AI Cloud exports](https://docs.nebius.com/signup-billing/usage/export), [CoreWeave billing](https://docs.coreweave.com/products/inference/billing), and [Foundry cost management](https://learn.microsoft.com/en-us/azure/foundry/concepts/manage-costs).

An authoritative total and an authoritative per-call breakdown are different things. All of these services charge real money. The question is which identity dimensions survive into the provider's records, at what granularity, and whether the records have been reconciled with the invoice. A running usage report may still change before billing closes.

Nebius documents its AI Cloud export as FOCUS 1.2, with conformance gaps described separately. Treat that as the current export format, not a guarantee of coverage for a different Nebius service. Token Factory also has monitoring integrations; monitoring labels alone do not prove a billing join.

Sources: [export documentation](https://docs.nebius.com/signup-billing/usage/export), [Token Factory observability](https://docs.tokenfactory.nebius.com/ai-models-inference/observability), and [monitoring integration](https://docs.tokenfactory.nebius.com/ai-models-inference/observability-api-integrations).

## Try the request path

For Nebius:

```sh
cargo run -p gatewayd -- --config deploy/examples/nebius.yaml --listen 127.0.0.1:8080
```

In another terminal, with your provider key already set in `NEBIUS_API_KEY`:

```sh
curl --no-buffer http://127.0.0.1:8080/v1/chat/completions \
  -H "Authorization: Bearer $NEBIUS_API_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"model":"YOUR_MODEL_ID","messages":[{"role":"user","content":"Hello"}],"stream":true,"stream_options":{"include_usage":true}}'
```

Choose a model available in your project. The endpoint and authentication follow the [Nebius quickstart](https://docs.tokenfactory.nebius.com/quickstart).

For CoreWeave, replace `your-gateway.example.com` in `upstream.host` in `deploy/examples/coreweave.yaml` with the hostname from your gateway's `status.endpoints`. Then:

```sh
cargo run -p gatewayd -- --config deploy/examples/coreweave.yaml --listen 127.0.0.1:8080
```

Send the same request using your CoreWeave token and the model name configured on that deployment. The example assumes `coreWeaveAuth` and body-based routing. CoreWeave documents Dedicated Inference as a private preview, so access depends on your account. See [getting started](https://docs.coreweave.com/products/inference/getting-started) and [gateway configuration](https://docs.coreweave.com/products/inference/gateways).

Both recipes retain `/v1/chat/completions`. The gateway sets the upstream Host header from the destination because the public endpoint has a different hostname from the local gateway. Authentication passes through to the provider; no secret is stored in the examples.

These are local, single-team recipes. The team and application are assigned by the operator for all traffic using that configuration. They do not authenticate an individual user. For a shared gateway, authenticate callers and derive ownership from verified claims or controlled policy. A shared provider API key cannot identify which person used it.

For streaming calls, request `stream_options.include_usage` where the endpoint supports it. Verify the terminal usage frame on your deployed model. For ordinary JSON calls, omit the streaming fields; the JSON tap reads the response's usage object. Missing usage stays unknown. Neither path produces an invoice charge.

## Scope

These recipes cover request forwarding, attribution, and token metering through the existing adapter. Importing invoices and allocating GPU costs are outside smallgateway's current scope. Use the provider's billing records and your own cost-reporting tools for that work.
