# Grafana Dashboard Setup

> [!WARNING]
>
> The dashboard is currently unstable and could even be broken while subcoin is still being actively developed. 

## Prometheus Metrics Setup

By default, the Subcoin node exposes Prometheus metrics at `localhost:9615`. If you change this endpoint, ensure that you update the prometheus.yml configuration file accordingly.

## Start Prometheus and Grafana Services

To start the Prometheus and Grafana services, use Docker Compose:

```bash
docker-compose up -d
```

Once the services are running, open Grafana by navigating to `http://localhost:3000` in your web browser. You can then import the dashboard.

## Troubleshooting No Data Issues

> Get "http://localhost:9615/metrics": dial tcp 127.0.0.1:9615: connect: connection refused

Open http://localhost:9090/targets in your web browser. If you see an error, edit the prometheus.yml file and replace:

```yml
targets: ["localhost:9615"]
```

with:

```yml
targets: ["host.docker.internal:9615"]
```

> Post "http://localhost:9090/api/v1/query": dial tcp 127.0.0.1:9090: connect: connection refused - There was an error returned querying the Prometheus API.

When running Docker on macOS, `http://localhost:9090` may not work. Instead, try accessing Prometheus via `http://prometheus:9090`. For more details, refer to [this GitHub issue](https://github.com/grafana/grafana/issues/46434).
