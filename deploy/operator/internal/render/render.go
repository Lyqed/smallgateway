// Package render translates an LLMGateway spec into the gatewayctl config-repo
// layout — the SAME fragment layout crates/gatewayctl/src/render.rs reads and
// compiles. The operator deliberately does NOT render the flat gateway config
// or reimplement scope composition: it writes repo fragments, mounts them into
// a gatewayctl Deployment via --repo, and lets the tested control-plane
// pipeline compose + validate + hash + distribute. That keeps one authority for
// config semantics (gateway-core, through gatewayctl) and one for topology (this
// operator).
//
// Repo layout produced (see render.rs "The repo layout"):
//
//	providers.yaml
//	rejections.yaml            (GB-4, required)
//	fleet/base.chain.yaml      (optional)
//	projects/<p>/base.chain.yaml
//	routes/<name>.route.yaml   (one per route, sorted by filename)
//	budget.yaml                (GB-5 caps; consumed by gatewayctl budget config)
//
// GB-2 JWT auth is project-deferred and is NOT rendered here (no auth.yaml);
// the v1alpha1 spec exposes no auth field. See README "Follow-ups".
package render

import (
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"sort"

	"sigs.k8s.io/yaml"

	gwv1 "github.com/thegatewayproject/gateway-operator/api/v1alpha1"
)

// Fragment is one repo file: a relative path and its bytes.
type Fragment struct {
	Path  string
	Bytes []byte
}

// Result is the rendered repo plus its content hash.
type Result struct {
	Fragments []Fragment
	// Hash is a SHA-256 over the sorted (path, bytes) pairs — the operator's
	// config-input identity. It changes iff the desired config changes. It is
	// NOT the gatewayctl render_hash (which is computed over the composed flat
	// bytes); it is the input hash the operator can compute without replicating
	// gateway-core's canonicalization. Documented as such in the CR status.
	Hash string
}

// Render turns a spec into the config-repo fragments and their hash.
func Render(spec *gwv1.LLMGatewaySpec) (*Result, error) {
	var frags []Fragment

	// providers.yaml (required)
	provs := map[string]any{}
	for name, p := range spec.Providers {
		up := map[string]any{"host": p.Upstream.Host, "port": p.Upstream.Port}
		if p.Upstream.TLS {
			up["tls"] = true
		}
		if p.Upstream.SNI != "" {
			up["sni"] = p.Upstream.SNI
		}
		provs[name] = map[string]any{"kind": p.Kind, "upstream": up}
	}
	b, err := yaml.Marshal(provs)
	if err != nil {
		return nil, fmt.Errorf("providers: %w", err)
	}
	frags = append(frags, Fragment{Path: "providers.yaml", Bytes: b})

	// rejections.yaml (GB-4, required by the data plane). If the spec omits a
	// reason, supply a conservative JSON default so the render always validates.
	rej := renderRejections(spec.Rejections)
	b, err = yaml.Marshal(rej)
	if err != nil {
		return nil, fmt.Errorf("rejections: %w", err)
	}
	frags = append(frags, Fragment{Path: "rejections.yaml", Bytes: b})

	// fleet/base.chain.yaml (optional scope 1)
	if spec.Fleet != nil && spec.Fleet.Attribution != nil {
		chain := map[string]any{"attribution": renderAttribution(spec.Fleet.Attribution)}
		b, err = yaml.Marshal(chain)
		if err != nil {
			return nil, fmt.Errorf("fleet chain: %w", err)
		}
		frags = append(frags, Fragment{Path: "fleet/base.chain.yaml", Bytes: b})
	}

	// projects/<p>/base.chain.yaml (optional scope 2)
	projNames := make([]string, 0, len(spec.Projects))
	for name := range spec.Projects {
		projNames = append(projNames, name)
	}
	sort.Strings(projNames)
	for _, name := range projNames {
		sc := spec.Projects[name]
		chain := map[string]any{}
		if sc.Attribution != nil {
			chain["attribution"] = renderAttribution(sc.Attribution)
		}
		b, err = yaml.Marshal(chain)
		if err != nil {
			return nil, fmt.Errorf("project %s chain: %w", name, err)
		}
		frags = append(frags, Fragment{Path: fmt.Sprintf("projects/%s/base.chain.yaml", name), Bytes: b})
	}

	// routes/<name>.route.yaml (required; one per route)
	for _, r := range spec.Routes {
		route := map[string]any{"prefix": r.Prefix, "provider": r.Provider}
		if r.Project != "" {
			route["project"] = r.Project
		}
		if r.Match != "" {
			route["match"] = r.Match
		}
		if r.Attribution != nil {
			route["attribution"] = renderAttribution(r.Attribution)
		}
		b, err = yaml.Marshal(route)
		if err != nil {
			return nil, fmt.Errorf("route %s: %w", r.Name, err)
		}
		frags = append(frags, Fragment{Path: fmt.Sprintf("routes/%s.route.yaml", r.Name), Bytes: b})
	}

	// NOTE: no auth.yaml is rendered. GB-2 JWT auth is project-deferred and is
	// not exposed in the v1alpha1 spec, so there is nothing to resolve here. It
	// will be added only when the Secret->auth.yaml resolution is implemented
	// and verified end to end (see README "Follow-ups").

	// budget.yaml (GB-5 caps). gatewayctl reads a budget config; the caps map
	// straight onto attributed-spend ceilings.
	if spec.SpendCaps != nil && len(spec.SpendCaps.Caps) > 0 {
		caps := make([]map[string]any, 0, len(spec.SpendCaps.Caps))
		for _, c := range spec.SpendCaps.Caps {
			window := c.Window
			if window == "" {
				window = "day"
			}
			caps = append(caps, map[string]any{
				"key":       c.Key,
				"value":     c.Value,
				"limit_usd": c.LimitUsd,
				"window":    window,
			})
		}
		b, err = yaml.Marshal(map[string]any{"caps": caps})
		if err != nil {
			return nil, fmt.Errorf("budget: %w", err)
		}
		frags = append(frags, Fragment{Path: "budget.yaml", Bytes: b})
	}

	// Stable order for hashing and ConfigMap key layout.
	sort.Slice(frags, func(i, j int) bool { return frags[i].Path < frags[j].Path })

	h := sha256.New()
	for _, f := range frags {
		h.Write([]byte(f.Path))
		h.Write([]byte{0})
		h.Write(f.Bytes)
		h.Write([]byte{0})
	}
	return &Result{Fragments: frags, Hash: hex.EncodeToString(h.Sum(nil))}, nil
}

func renderAttribution(a *gwv1.Attribution) map[string]any {
	out := map[string]any{}
	if len(a.RequiredKeys) > 0 {
		out["required_keys"] = a.RequiredKeys
	}
	if len(a.Pinned) > 0 {
		out["pinned"] = a.Pinned
	}
	return out
}

func renderRejections(r *gwv1.Rejections) map[string]any {
	missing := defaultMissing
	unknown := defaultUnknown
	if r != nil {
		if r.MissingAttribution != nil {
			missing = renderTemplate(r.MissingAttribution)
		}
		if r.UnknownRoute != nil {
			unknown = renderTemplate(r.UnknownRoute)
		}
	}
	return map[string]any{
		"missing_attribution": missing,
		"unknown_route":       unknown,
	}
}

func renderTemplate(t *gwv1.RejectionTemplate) map[string]any {
	out := map[string]any{
		"status":       t.Status,
		"content_type": t.ContentType,
		"body":         t.Body,
	}
	if t.Streaming != nil {
		s := map[string]any{"data": t.Streaming.Data}
		if t.Streaming.Event != "" {
			s["event"] = t.Streaming.Event
		}
		out["streaming"] = s
	}
	return out
}

// Conservative GB-4 defaults, used when the CR omits a reason. Placeholders
// {{key}} / {{route}} are honored by the data plane at rejection time.
var defaultMissing = map[string]any{
	"status":       428,
	"content_type": "application/json",
	"body":         `{"error":"attribution_required","missing":"{{key}}","route":"{{route}}"}`,
	"streaming": map[string]any{
		"event": "error",
		"data":  `{"error":"attribution_required","missing":"{{key}}"}`,
	},
}

var defaultUnknown = map[string]any{
	"status":       404,
	"content_type": "application/json",
	"body":         `{"error":"unknown_route","path":"{{route}}"}`,
}
