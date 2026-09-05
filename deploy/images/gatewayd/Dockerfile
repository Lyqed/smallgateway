# Data-plane smoke-test image for smallgateway.
#
# Ships the PREBUILT release binaries (gatewayd + mock_upstream) rather than
# compiling in-image: the binaries are glibc-linked (wasmtime + pingora), so
# the base must be Debian/glibc, NOT musl. bookworm-slim carries glibc 2.36;
# the binaries need at most GLIBC_2.34, so the ABI matches the build host.
#
# The same image runs in two roles in the chart:
#   - default CMD: gatewayd, the proxy under test
#   - override:    mock_upstream, the in-cluster fake provider (sidecar)
FROM debian:bookworm-slim

# ca-certificates only; the binaries are otherwise self-contained.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Prebuilt binaries.
COPY gatewayd       /usr/local/bin/gatewayd
COPY mock_upstream  /usr/local/bin/mock_upstream

# Streaming fixtures the mock replays (baked in so the mock needs no volume).
COPY fixtures/ /opt/gateway/fixtures/

# A baked fallback config so `docker run` works without a mounted config.
# In the cluster the chart's ConfigMap overrides this at /etc/gateway.
COPY gateway.yaml /etc/gateway/gateway.yaml

# gatewayd listen port (the chart's Service targets this).
EXPOSE 8080

# Run as an unprivileged user; nothing here needs root at runtime.
USER nobody

# Default role: the proxy, reading its config file and listening on all
# interfaces so the in-cluster Service can reach it.
ENTRYPOINT ["/usr/local/bin/gatewayd"]
CMD ["--config", "/etc/gateway/gateway.yaml", "--listen", "0.0.0.0:8080"]
