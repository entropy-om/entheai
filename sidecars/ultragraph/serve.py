#!/usr/bin/env python3
"""ultra-graph rerank sidecar — Stage 2 of prompt-processing (Slice 2).

A stateless stdio JSON-RPC server. entheai's Rust `SidecarMesh` spawns this per
prompt, writes one `rerank` request line to stdin, reads one response line from
stdout, then closes stdin (EOF) — we respond and exit.

Protocol
--------
request  : {"jsonrpc":"2.0","id":<n>,"method":"rerank",
            "params":{"query":<str>,"spans":[{"id":<str>,"text":<str>},...],
                      "deadline_ms":<int>,"top_k":<int>}}
response : {"jsonrpc":"2.0","id":<n>,"result":{"ranked_span_ids":[<id>,...]}}
on error : {"jsonrpc":"2.0","id":<n>,"error":{"code":<int>,"message":<str>}}

The sidecar returns **ids only** — never rewritten text. The Rust side rehydrates
the raw payloads by id, preserving "never returns a rewritten payload".

Ranking
-------
If the user's `ultragraph` package (a BitNet-b1.58 ternary "1-bit LLM" mesh) is
importable, we rank with it. Otherwise we fall back to a deterministic lexical
reference scorer (query-term overlap, stable by input order) so the sidecar is
useful and testable without the model installed. Either way the Rust client's
strict deadline is the real guard: if we're slow or crash, entheai falls back to
top-K retrieval with no regression.

STDOUT carries the JSON response ONLY. All diagnostics go to STDERR.
"""

import json
import re
import sys

_TOKEN = re.compile(r"[a-z0-9]+")


def _log(msg: str) -> None:
    print(f"[ultragraph.serve] {msg}", file=sys.stderr, flush=True)


def _try_ultragraph():
    """Return a callable rerank(query, spans)->[ids] backed by ultragraph, or None."""
    try:
        import ultragraph  # noqa: F401  (the user's 1-bit mesh package)
    except Exception:
        return None

    def _rank(query, spans):
        # ultragraph exposes a byte-graph BitNet model; we score each span's
        # relevance to the query with the mesh and sort descending. The exact
        # entrypoint is versioned, so guard it and fall through on any mismatch.
        mesh = ultragraph.Mesh()  # type: ignore[attr-defined]
        scored = [(s["id"], mesh.relevance(query, s.get("text", ""))) for s in spans]
        scored.sort(key=lambda t: t[1], reverse=True)
        return [sid for sid, _ in scored]

    return _rank


def _lexical_rerank(query, spans):
    """Deterministic fallback: rank by count of distinct query terms present.

    Stable — ties keep input order — so output is reproducible (golden-testable).
    """
    terms = set(_TOKEN.findall(query.lower()))

    def score(span):
        text = span.get("text", "").lower()
        present = {t for t in terms if t in text}
        return len(present)

    order = sorted(range(len(spans)), key=lambda i: (-score(spans[i]), i))
    return [spans[i]["id"] for i in order]


def handle(req, rank):
    if req.get("method") != "rerank":
        raise ValueError(f"unknown method {req.get('method')!r}")
    params = req.get("params") or {}
    spans = params.get("spans") or []
    query = params.get("query") or ""
    ranked = rank(query, spans)
    top_k = params.get("top_k")
    if isinstance(top_k, int) and top_k > 0:
        ranked = ranked[:top_k]
    return {"ranked_span_ids": ranked}


def main() -> int:
    rank = _try_ultragraph()
    _log("ranking via ultragraph" if rank else "ranking via lexical fallback (ultragraph absent)")
    if rank is None:
        rank = _lexical_rerank

    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        rid = None
        try:
            req = json.loads(line)
            rid = req.get("id")
            result = handle(req, rank)
            resp = {"jsonrpc": "2.0", "id": rid, "result": result}
        except Exception as exc:  # never crash the caller — emit a JSON-RPC error
            resp = {"jsonrpc": "2.0", "id": rid, "error": {"code": -32000, "message": str(exc)}}
        sys.stdout.write(json.dumps(resp) + "\n")
        sys.stdout.flush()
    return 0


if __name__ == "__main__":
    sys.exit(main())
