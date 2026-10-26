# 0027: Numeric Android phase spans

Use a dependency-free internal telemetry crate shared by store, agent and
HTTP client. Android alog writes only fixed phase labels and numeric
microseconds; no URLs, models, headers, keys, request bodies or tool output.
Host phase logging is disabled. Existing OVERHEAD records now retain
microsecond resolution instead of rounding sub-millisecond work away.

persist measures a synchronous store barrier including queue wait and commit;
PERSIST_BATCH records actual batch size and SQL/commit duration separately.
request_build includes request cloning and JSON value construction;
serialize covers JSON encoding; dispatch covers HTTP request construction
and handing it to reqwest. connect is the observable execute-to-response-
headers interval, including request transmission and server header wait,
not an isolated TCP handshake. Reqwest does not expose handshake duration
through this API. Pretending the interval is pure TCP connect was rejected.

OVERHEAD remains the end-of-tool-stream to next-request-dispatch gap. It
includes tool and permission work but excludes the next request's network
wait. Phase totals cover all requests in a turn, so they must not be summed
and presented as that single gap. Timing logs add their own small overhead.
