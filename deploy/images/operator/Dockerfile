# gateway-operator image: the Kubernetes controller for the LLMGateway CRD.
#
# Built from the Go source under deploy/operator. Multi-stage: compile a static
# binary, then ship it on a distroless base (no shell, minimal attack surface).
# The build context is deploy/operator (see deploy/images/build.sh).
FROM golang:1.21-bookworm AS build
WORKDIR /src
COPY go.mod go.sum ./
RUN go mod download
COPY . .
RUN CGO_ENABLED=0 GOOS=linux go build -trimpath -ldflags="-s -w" -o /out/gateway-operator .

FROM gcr.io/distroless/static-debian12:nonroot
COPY --from=build /out/gateway-operator /usr/local/bin/gateway-operator
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/gateway-operator"]
