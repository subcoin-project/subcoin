# Grafana

> [!WARNING]
>
> The dashboard is unstable and could even be broken while subcoin is still being actively developed. 

By default, the subcoin node exposes the prometheus metrics at `localhost:9615`. Update `prometheus.yml`
accordingly if you change the default prometheus endpoint.

Now start the prometheus and grafana service.

```bash
docker-compose up -d
```

Open `http://localhost:3000` and import the dashboard.

## No Data Troubleshooting

> Get "http://localhost:9615/metrics": dial tcp 127.0.0.1:9615: connect: connection refused

Open `http://localhost:9090/targets` if you see an error there, try replacing `targets: ["localhost:9615"]` with `targets: ["host.docker.internal:9615"]` in `prometheus.yml`.

> Post "http://localhost:9090/api/v1/query": dial tcp 127.0.0.1:9090: connect: connection refused - There was an error returned querying the Prometheus API.

When starting from Docker on macOS, `http://localhost:9090` may not work, try `http://prometheus:9090` then.

https://github.com/grafana/grafana/issues/46434
