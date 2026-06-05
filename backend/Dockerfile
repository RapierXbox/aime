FROM golang:1.26.4 AS build
WORKDIR /src
COPY go.mod go.sum ./
RUN go mod download
COPY . .
RUN CGO_ENABLED=0 GOOS=linux go build -ldflags="-s -w" -o /out/server ./cmd/server

FROM alpine:3.20
RUN apk add --no-cache ca-certificates && adduser -D -u 10001 app
COPY --from=build /out/server /usr/local/bin/server
USER app
EXPOSE 8080
ENTRYPOINT ["server"]