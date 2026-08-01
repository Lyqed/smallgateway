# Control-plane image for The Gateway Project: gatewayctl.
#
# Ships the PREBUILT release binary (glibc-linked, same as the gatewayd smoke
# image). bookworm-slim carries glibc 2.36; the binary needs at most GLIBC_2.34.
# The operator runs gatewayctl with `--repo /repo` fed from a ConfigMap the
# operator renders from the LLMGateway CR.
FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY gatewayctl /usr/local/bin/gatewayctl

# gatewayctl fleet gRPC listen port (the control-plane Service targets this).
EXPOSE 6187

# No USER nobody here: the operator's initContainer needs to write the emptyDir
# repo, and gatewayctl reads it; both run as the pod's default. The chart sets a
# restrictive securityContext instead. Entrypoint is the binary; the operator
# supplies --repo/--listen/--join-token args.
ENTRYPOINT ["/usr/local/bin/gatewayctl"]
