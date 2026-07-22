+++
name = "observability"
domain = "logs / metrics / traces — knowing what production is doing"
triggers = ["observability", "otel", "opentelemetry", "tracing", "metrics", "logs", "prometheus", "grafana", "sentry", "monitor", "alert"]
rank = 0.85
+++
If it runs unattended, you must be able to **see what it's doing**. Instrument from the
start, not after the first incident.

**Three signals, one standard — OpenTelemetry (OTel):**
- **Structured logs** (JSON, with a request/trace id) — greppable, not prose. Levels used
  honestly.
- **Metrics** (Prometheus/OTLP): RED for services (Rate, Errors, Duration) + USE for
  resources (Utilization, Saturation, Errors). Dashboards in Grafana.
- **Traces** (OTel spans): follow one request across services; the single best tool for
  "why is this slow / where did it fail."

**Practices:** propagate a trace/correlation id through every hop (and into fan-out
sub-agents); sample traces (head or tail) to bound cost; alert on **symptoms users feel**
(SLO burn, error rate, p99 latency), not every twitch — noisy alerts get ignored. Ship
crash/error reporting (Sentry) with real backtraces (keep symbols; don't strip in the
profile you debug). A fallback path should *log loudly* what it fell back from.
