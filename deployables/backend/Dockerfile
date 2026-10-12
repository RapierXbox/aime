FROM golang:1.26-bookworm AS build
WORKDIR /src
COPY go.mod go.sum ./
RUN go mod download
COPY . .
RUN CGO_ENABLED=1 go build -tags onnx -ldflags="-s -w" -o /out/server ./cmd/server

FROM debian:bookworm-slim
ARG TARGETARCH
ARG ORT_VERSION=1.29.1 # must match the api version in the go binding (onnxruntime_c_api.h ORT_API_VERSION)
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl && rm -rf /var/lib/apt/lists/* \
 && case "$TARGETARCH" in amd64) ORT_ARCH=x64;; arm64) ORT_ARCH=aarch64;; *) echo "unsupported arch $TARGETARCH" && exit 1;; esac \
 && curl -fsSL "https://github.com/microsoft/onnxruntime/releases/download/v${ORT_VERSION}/onnxruntime-linux-${ORT_ARCH}-${ORT_VERSION}.tgz" | tar xz -C /tmp \
 && cp /tmp/onnxruntime-*/lib/libonnxruntime.so* /usr/lib/ && rm -rf /tmp/onnxruntime-* \
 && adduser --disabled-password --gecos "" --uid 10001 app \
 && mkdir /backups && chown app /backups
ENV ONNXRUNTIME_LIB=/usr/lib/libonnxruntime.so
COPY --from=build /out/server /usr/local/bin/server
USER app
EXPOSE 8080
ENTRYPOINT ["server"]
