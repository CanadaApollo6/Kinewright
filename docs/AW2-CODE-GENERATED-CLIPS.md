# AW2: code-generated clips

Status: **design accepted 2026-10-15** (revision 3). Implementation has not started. It follows PF1 acceptance per §11.
Review trail, under `target/review/aw/`:
- Astra critic: `aw2-design-critic.md`.
- Verify of rev2: `aw2-design-verify.md`, "promote with edits".
- Narrow re-check of rev3: `aw2-design-verify-r2.md`, "promote".

Open for Riel (§13):
- approve the final fixtures, performance ceilings and Win11 VM configuration;
- accept the person-provisioned immutable-image setup burden (R24).

Main source baseline: `f589529`. PF1 integration baseline: `pf1/impl` at `c9dbe4d`. That baseline does not establish
PF1 acceptance.

Binding sources are `target/review/PITFALLS.md`, AW0 Amendment A1 and §§5/7/8/9/12/15, the roadmap AW2 row and
production order, the Astra critic, and the lead's round-2 rulings.
- Existing-code claims carry file:line or section citations.
- Rules R1-R31 are implementation contracts.
- Performance figures are provisional. Line counts are not a budget, cap, forecast or stop rule (Riel, 2026-10-20; §11).
- Accepted AW1/PF1 limits take precedence when stricter.

## 0. Part 0: AW2-OBL-1 opens the slice

### 0.1 Ownership and admitted messages

MO2 carries the malformed-id HTTP-202 and echoing -32020 obligations into AW2
(`docs/MO2-BLEND-ADJUSTMENT-TRANSITIONS-SOLIDS.md:59-92`). The solo response bound is downstream of transport decoding
(`crates/kinewright-agent/src/server.rs:11869-11896,14063-14094`). It cannot repair these pre-dispatch paths, inferred.
The rmcp pin is 3.1.2 (`Cargo.toml:38`); its HTTP dispatch accepts notifications, results and errors at
`rmcp-3.1.2/src/transport/streamable_http_server/tower.rs:1793-1801`. AW1 destructive consent explicitly needs a client
elicitation reply (`docs/AW1-HEADLESS-KINEWRIGHT.md` §8). Guarding only method-bearing requests would break it.

R1 [P0]. Install a shared byte/envelope guard before rmcp's untagged deserializer on HTTP and AW1 stdio. Parse keys
before typed dispatch, preserve presence separately from null, and reject duplicate decoded keys at every object
level. `"id"` and `"\u0069d"` are the same key. Reject trailing JSON, arrays/batches, nonobjects, invalid UTF-8,
wrong `jsonrpc`, unknown envelope keys and mixed shapes. Permitted envelope keys are exactly those in the selected row.
All rows require `jsonrpc:"2.0"`; params/result are objects; error is an object with integer code, string message and
optional JSON data. Nested method-specific fields remain the responsibility of rmcp's typed validator.

| Disjoint shape | Required and optional keys beyond jsonrpc | Admission and delivery |
| --- | --- | --- |
| Request | id, method string; optional params object | No result/error. Admit bounded id, then version and typed params. Dispatch exactly once. A duplicate outstanding client request id is -32600 and never displaces its first call |
| Notification | method string; optional params object; id absent | No result/error. Deliver a known, typed-valid notification once. Unknown well-formed notifications are ignored. No JSON-RPC response |
| Result | id, result object | No method/params/error. Admit only against an outstanding server-to-client request in the same authenticated peer and exchange |
| Error | error object; id required for correlated settlement, otherwise the modern diagnostic case below | No method/params/result. With an id, same correlation rule. A valid client error settles the outstanding call as an error; it is not a transport refusal |

R2 [P0, adopted Q1]. A present id must be an integer token within [-9223372036854775808,9223372036854775807], or a
string whose canonical JSON encoding occupies at most 256 UTF-8 bytes, quotes included. Integer syntax disallows
fractions and exponents, including `1.0` and `1e0`. Canonical string encoding means the pinned serde_json string writer,
UTF-8 for noncontrol Unicode, escaped quotes/backslashes/control characters, no optional ASCII escaping. This encoding
also defines the bounded correlation echo. Null/bool/object/array, overflow and over-cap strings receive -32600.
Invalid ids never fall back to notification admission. Structural failures, including duplicate keys and mixed shapes,
trust no id even if one individual token is legal. Do not truncate or hash ids for protocol correlation.

Response correlation uses rmcp's outstanding-call owner, extended at its admission seam rather than a second consent
broker. Key it by authenticated peer/exchange, direction and exact typed id; integer 1 differs from string "1".
Register before sending the server request; remove once on result, error, cancellation or expiry. Bound it to 64 entries
and 32 KiB per peer, with server-issued bounded ids and the originating request's deadline. New server requests refuse
locally when full. A replayed, unsolicited, wrong-peer, expired or already-settled response gets the nonresponse refusal
in §0.3; it never authorizes consent or generates another server request. Check the correlated result's expected type,
including elicitation accept/decline/cancel. Valid results/errors receive HTTP 202 with zero body or no stdio output.
The legacy transport explicitly allows response POSTs ([legacy transport, sending messages](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports)).
Modern multi-round-trip input continues through rmcp's modern request/result pattern; it is not converted into a legacy
elicitation response. Modern result validation includes its required resultType. The modern error shape also permits an absent id when the
peer could not read a request id (`model.rs:501-510`). Admit that shape solely as an uncorrelatable peer diagnostic,
accept and discard it with HTTP 202/no stdio output, and count only a fixed local event. It never settles a pending
call, authorizes consent or establishes request context. A present null or otherwise invalid id still gets -32600.
This exception is disjoint from a result, which always requires a correlated id.

### 0.2 Per-request version and error-shape precedence

R3 [P0]. Version is request context, never a modern connection-wide negotiated value. The authoritative fields are
`params.protocolVersion` on legacy initialize, otherwise `params._meta["io.modelcontextprotocol/protocolVersion"]`.
The HTTP mirror is `MCP-Protocol-Version`. Modern statelessness and legacy initialize coexist in the SDK
(`tower.rs:354-406,2091-2118`; [versioning](https://modelcontextprotocol.io/specification/2026-07-28/basic/versioning)).
Use the following algorithm, including for refusals. Candidate error syntax selection is separate from refusal-code
selection. R4's ordered phases determine the code; a malformed body version does not skip an earlier HTTP mirror check.

1. Before a complete unambiguous envelope exists, only a single well-formed HTTP version header may select modern
   error syntax. Otherwise use legacy no-id syntax. Do not mine a partial body or a rejected/duplicated key for version.
2. On an admitted initialize request with a string body version, use that version for syntax. A present, well-formed
   differing header is an initialize mismatch, -32600, before either value can be logged. An absent header is permitted
   for legacy initialize; a present malformed header is -32020. An invalid body version type is -32602 only after
   the applicable header checks pass; never compare a malformed body field as a version string. A supported modern
   declaration on initialize is -32601 under modern syntax; initialize selects legacy only. Legacy negotiation remains
   rmcp-owned. Without a usable body string, a valid header or discover selects syntax, otherwise use legacy syntax.
3. On another admitted request, a present, well-typed body version wins for syntax selection. HTTP must then carry the
   same version header. Missing, malformed or conflicting required mirrored headers give -32020. A malformed body
   version field gives -32602 only if earlier required HTTP mirror checks pass. If a body version key is present but
   its value is not a string, or _meta itself is ill-typed, that request still requires the protocol mirror on HTTP.
   Missing/malformed mirrors therefore give -32020 first. A valid header cannot be compared for equality with a
   malformed body field; defer that comparison and return -32602 after other mirror checks pass. On stdio there are no
   mirror checks, so the malformed field gives -32602 directly. A header/discover may select modern error syntax in this
   case; otherwise use legacy error syntax, without admitting a legacy request. Conflicts never dispatch.
4. Without a body version, a valid HTTP header selects that request's candidate. A modern candidate, or
   `server/discover`, requires modern metadata. On HTTP, missing/malformed required mirrors give -32020 first; after
   they pass, missing or ill-typed body metadata gives -32602. On stdio,
   discover is modern even without its required metadata. Modern requests require a protocol version and a well-typed
   clientCapabilities object on every request. Do not borrow them from a previous call.
5. With neither declaration, only an established legacy peer may supply its negotiated version, then HTTP's legacy
   compatibility default is 2025-03-26. Pre-initialize stdio has no valid legacy request context except initialize;
   refuse other non-discover requests with -32600. A legacy session's explicit header must match its negotiated version;
   otherwise -32020. A complete modern body declaration bypasses legacy session version/capabilities, even on the same
   connection. Test modern/legacy interleaving explicitly.
6. Results/errors inherit the version of their correlated outstanding server request, not the latest client call.
   Notifications use their originating legacy peer or explicit supported modern HTTP header for validation only;
   they never create a modern session. Stateless cancellation uses its explicit request context in rmcp.
7. For an unsupported version on a non-initialize request, select modern syntax if it was a per-request body declaration,
   discover, or an ISO date at least 2026-07-28; otherwise legacy syntax. After consistency checks, refuse -32022.
   Publish only this fixed supported list, never the supplied version:
   `["2024-11-05","2025-03-26","2025-06-18","2025-11-25","2026-07-28"]`.
   R4 deliberately omits data.requested from this error; this is an explicit schema compatibility deviation for the
   binding AW2-OBL-1 echo-free obligation, not full 2026-07-28 conformance. The fixed supported list matches rmcp's default
   (`model.rs:181-187`, `handler/server.rs:332-341`). If accepted AW1 narrows it,
   revise this literal and its wire fixtures before P0 starts; never interpolate a request-supplied value.
   An unsupported legacy initialize proposal remains rmcp's negotiation case: it can return a supported counterproposal
   successfully. It is not the unsupported per-request-version refusal. Preserve and test that successful legacy control.

For a missing/untrustworthy id, legacy error syntax includes `"id":null`; modern syntax omits the id member.
An admitted request id is the only input-derived output allowed by the transport refusal contract. HTTP parser,
authentication and routing refusals do not inspect the body and have no JSON-RPC correlation. Valid client error
payloads are delivered to their caller, never copied into logs or turned into an outbound refusal.

### 0.3 Complete refusal constructors and precedence

R4 [P0]. These templates specify literal UTF-8 bytes, key order and whitespace. Angle-bracket slots below are grammar
slots, not output. `I` is canonical admitted-id JSON or legacy `null`. The modern no-id form omits the whole id member.
`C`, `M` and `T` come only from the table. The trailing newline occurs only on stdio.

- RPC with id: `{"jsonrpc":"2.0","id":<I>,"error":{"code":<C>,"message":"<M>","data":{"code":"<T>"}}}`
- RPC without id: `{"jsonrpc":"2.0","error":{"code":<C>,"message":"<M>","data":{"code":"<T>"}}}`
- HTTP-only body: `{"error":"<T>"}`. No newline.
- Example invalid legacy request: `{"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"Invalid request","data":{"code":"transport_invalid_request"}}}`
- Example modern mismatch: `{"jsonrpc":"2.0","id":1,"error":{"code":-32020,"message":"Header mismatch","data":{"code":"transport_header_mismatch"}}}`
- After SSE headers have committed: exactly `event: message\ndata: <RPC>\n\n`, where each `\n` denotes one LF byte.
  Emit one refusal event, then terminate that request's stream. No input-derived SSE id, retry, comment or diagnostic event.

All HTTP refusals close the HTTP/1 connection after the fixed response. They never drain an unbounded rejected body.
A request whose SSE response already started keeps status 200 and closes its stream after the one bounded error event;
other peers/sessions are unaffected. No partial successful result precedes a replacement error. If the socket cannot
write, close it and record the fixed local event; delivery is not claimed. HTTP/2, upgrades, CONNECT and TLS termination
inside this local listener are outside AW2's admitted transport. Refuse an attempted HTTP/2 preface at the parser.

| Refusal condition, evaluated in numbered phase order | HTTP status | Body / RPC C, M, T | Stdio disposition |
| --- | --- | --- | --- |
| 1. HTTP head syntax, invalid request target, conflicting Content-Length/Transfer-Encoding | 400; unsupported HTTP version 505 | HTTP-only `transport_http_syntax`; version uses `transport_http_version` | Not applicable |
| 0. More than 32 simultaneously admitted HTTP connections | 503 | HTTP-only `transport_connection_limit` | Not applicable |
| 1. Head exceeds 16,384 bytes or 128 fields | 431 | HTTP-only `transport_header_limit` | Not applicable |
| 1. Head acquisition exceeds 5 s | 408 | HTTP-only `transport_read_timeout` | Not applicable |
| 2. Missing/invalid authentication | 401 | HTTP-only `transport_unauthorized` | AW1 startup auth failure: no stdout, fixed stderr event, exit 1 |
| 2. Host/authority or Origin fails allowlist | 403 | HTTP-only `transport_forbidden` | Not applicable |
| 3. Route other than /mcp, or routing failure | 404 | HTTP-only `transport_route_not_found` | Not applicable |
| 3. Unsupported HTTP verb / legacy-only GET or DELETE on modern request | 405 | HTTP-only `transport_method_not_allowed` | Not applicable |
| 3. Missing/incompatible Accept | 406 | HTTP-only `transport_not_acceptable` | Not applicable |
| 3. Non-JSON content type, nonidentity Content-Encoding | 415 | HTTP-only `transport_content_type` | Not applicable |
| 3. Unsupported Expect header | 417 | HTTP-only `transport_expectation` | Not applicable |
| 4. Four body readers already held; fifth is rejected before body polling | 503 | HTTP-only `transport_reader_limit` | Before acquiring frame: -32600, `Invalid request`, `transport_reader_limit`; emit once and close |
| 4. Declared or measured decoded entity/frame bytes > 8,388,608 | 413 | HTTP-only `transport_body_limit` | -32600, `Invalid request`, `transport_body_limit`; once, close |
| 4. Body/frame deadline expires, including unfinished frame | 408 | HTTP-only `transport_read_timeout` | -32600, `Invalid request`, `transport_read_timeout`; once, close |
| 4. Malformed HTTP chunks/trailers observed while reading | 400 | HTTP-only `transport_http_syntax` | Not applicable |
| 4. Body/pipe read fails, excluding clean EOF between frames and chunk syntax errors | 500 | HTTP-only `transport_read_failed` | -32603, `Internal error`, `transport_read_failed`; once if writable, close |
| 5. JSON syntax/UTF-8 failure, including truncated JSON at EOF | 400 | -32700, `Parse error`, `transport_parse_error`; no id | Once, close |
| 5. Depth > 64, duplicate keys, malformed envelope/id, batch/scalar, mixed shapes, duplicate pending request id | 400 | -32600, `Invalid request`, `transport_invalid_request`; no id | Once, close, so malformed response loops cannot continue |
| 6. Uncorrelated, duplicate or typed-invalid result/error with otherwise admitted envelope/id | 400 | HTTP-only `transport_invalid_response` | No JSON-RPC reply; fixed local event, fail affected pending call, close |
| 6. Known notification has invalid typed params | 400 | HTTP-only `transport_invalid_notification` | No stdout; fixed local event, discard frame, continue |
| 7. initialize header/body version mismatch | 400 | -32600, `Invalid request`, `transport_initialize_mismatch`; admitted id | RPC once; continue |
| 7. Required mirrored protocol/method/name/annotated parameter header absent, malformed or unequal | 400 | -32020, `Header mismatch`, `transport_header_mismatch`; admitted request id; notification uses HTTP-only `transport_header_mismatch`, GET/DELETE use no-id RPC | No HTTP mirrors on stdio; not applicable |
| 7. Unsupported per-request protocol version or unsupported explicit GET/DELETE version | 400 | -32022, `Unsupported protocol version`, `transport_protocol_unsupported`; admitted request id or no-id for GET/DELETE; notifications use HTTP-only `transport_protocol_unsupported`; fixed supported addition below | RPC once; continue |
| 7. Missing/malformed required request metadata or typed method params | 400 before dispatch; legacy dispatch errors 200 | -32602, `Invalid params`, `transport_invalid_params`; admitted request id | RPC once; continue |
| 8. Legacy request requires initialize or session id but lacks it | 400; preinitialize wrong message 422 | HTTP-only `transport_session_required`; 422 uses `transport_initialize_required` | -32600, `Invalid request`, `transport_initialize_required`; continue |
| 8. Unknown, terminated or unusable legacy session | 404 | HTTP-only `transport_session_not_found` | Fixed local event, close legacy peer |
| 8. SSE resume failure / missing required resume id | 200 empty SSE / 400 | Empty stream with no event or HTTP-only `transport_resume_required` | Not applicable |
| 9. Unknown request method | modern 404; legacy 200 | -32601, `Method not found`, `transport_method_not_found`; admitted id | RPC once; continue |
| 9. Required client capability absent | modern 400; legacy 200 | -32021, `Missing required client capability`, `transport_capability_required`; admitted id; fixed requiredCapabilities addition below | RPC once; continue |
| 10. Session manager, serialization, worker, unexpected transport variant, panic boundary or unmapped internal failure | 500 before stream | -32603, `Internal error`, `transport_internal_error`; admitted request id only if held | Once, close on broken transport; otherwise continue |
| 10. Refusal serializer/output budget violation | 500 before stream | -32603, `Internal error`, `transport_internal_error`; no id fallback | Emit fallback once, close |

For -32022, data is exactly `{"code":"transport_protocol_unsupported","supported":<S>}`, where S is exactly
`["2024-11-05","2025-03-26","2025-06-18","2025-11-25","2026-07-28"]` from R3. For -32021, data is exactly
`{"code":"transport_capability_required","requiredCapabilities":<K>}`. K is a compile-time constant owned by the
request method, for example `{"elicitation":{"form":{}}}` for form elicitation or `{"tasks":{}}` for tasks.
No client capabilities or supplied version are echoed. An unsupported capability path without a fixed registered K
maps to method-not-found instead of manufacturing an error from input. Method selection chooses among fixed literals.

Compatibility choice: **documented omission of data.requested**. The MCP 2026-07-28
[UnsupportedProtocolVersionError schema](https://modelcontextprotocol.io/specification/2026-07-28/schema#unsupportedprotocolversionerror)
requires requested as a string. AW2 deliberately omits it to preserve AW2-OBL-1's binding rule that only the admitted
correlation id may echo input. A bounded/escaped copy would still echo; a fabricated requested string would misstate
what the client sent. This constructor is therefore not fully conformant to that specialized schema. Do not advertise
full conformance or silently replace it with an echo. G0e below pins compatible-client handling and strict-schema
rejection. Clients requiring the specialized schema are unsupported for this refusal path until the binding contract
changes; a successful supported-version request remains a separate positive control.

HTTP refusal status lines are exactly HTTP/1.1 plus the listed code and one of these literal phrases:
`200 OK`, `400 Bad Request`, `401 Unauthorized`, `403 Forbidden`, `404 Not Found`, `405 Method Not Allowed`,
`406 Not Acceptable`, `408 Request Timeout`, `413 Payload Too Large`, `415 Unsupported Media Type`,
`417 Expectation Failed`, `422 Unprocessable Entity`, `431 Request Header Fields Too Large`,
`500 Internal Server Error`, `503 Service Unavailable`, `505 HTTP Version Not Supported`, followed by CRLF.
The fixed headers then use lowercase `content-type: application/json`, `content-length: <decimal body byte length>`, `cache-control: no-store`, `connection: close`, in that
order, with CRLF separators and final CRLF. 401 adds only `www-authenticate: Bearer`; 405 adds only the fixed configured
`allow: GET, POST, DELETE` for legacy mode or `allow: POST` for modern-only mode. Disable automatic Date, Server,
Location, diagnostic and request-id headers on these responses. No session id is created by a refusal. Empty accepted
POSTs are status 202 with zero body, and valid notifications produce no stdio bytes. GET success and DELETE success
retain their existing protocol behavior. The empty-resume row is the sole refusal-like HTTP-200 exception: it uses
`content-type: text/event-stream`, `content-length: 0`, `cache-control: no-store`, `connection: close`, with no events.
Accept no automatic 100 Continue before authentication and reader admission; other Expect values receive 417.
Unknown well-formed notifications remain accepted and ignored. A notification's header validation failure is an
HTTP-only error, never a JSON-RPC error response. Unsupported HTTP versions, including an HTTP/2 preface, use 505.

Precedence is phase number, then acquisition order for irreversible byte/read failures. Within phase 5, invalid UTF-8
or JSON syntax wins over structural validation; once the depth bound is crossed stop and report -32600 without reading
unbounded input. A full structurally valid request uses phase 7 in this exact order:

1. For initialize only, conflicting well-typed body/header versions give -32600.
2. Required HTTP mirrors, in order protocol version, method, name, then annotated parameter headers in sorted registered
   parameter order. Missing/malformed headers or a mismatch against a well-typed body value give -32020. A present body
   version key, ill-typed _meta, a modern header candidate or server/discover makes the protocol mirror required even
   when the body version is malformed/missing. No string equality check uses an ill-typed body value.
3. A well-typed unsupported per-request version gives -32022, using R3's syntax selection. An ill-typed version is not
   an unsupported string and does not enter this step.
4. Missing/malformed required body metadata, including a nonstring version, gives -32602. Check version before
   clientCapabilities; absent modern metadata never succeeds via a legacy fallback.
5. Typed method params give -32602. Later method/capability checks retain phases 9 and 10.

On stdio skip step 2; initialize header conflict is inapplicable. These steps supersede any unconditional malformed
version reading of R3. Authentication precedes body parsing;
therefore an unauthenticated malformed-id body receives 401 without inspection. If bytes exceed a limit exactly at the
deadline, limit wins when the reader has already observed them; otherwise timeout wins. Multi-fault fixtures pin these
choices. Domain tool refusals after successful transport admission keep their own contracts; raw transport failures
never pass through a tool-result formatter.

R5 [P0, patch and bounds]. Choose a narrow dependency patch plus agent-owned admission, not response-only middleware.
Inventory and replace the following actual constructors before P0 closes:

| Inspected boundary | Existing owners | Required change |
| --- | --- | --- |
| Listener/parser/router | `crates/kinewright-agent/src/server.rs:965-1024`, axum serving /mcp; hyper 1.11.0 pin at `Cargo.lock:1924-1927` | Own the HTTP/1 connection builder, head deadline, 16 KiB buffer and 128-field caps. Capture parser refusals before wire output; patch its error-response hook where the service cannot see them. Parser EOF/reset writes nothing if no response is possible |
| Header and protocol constructors | rmcp `tower.rs:259-290,354-406,411-454,464-609,654-726,827-910` | Map all status/body constructors to R4 and replace dynamic diagnostic construction before tracing |
| POST/GET/DELETE, session, discovery, resume | rmcp `tower.rs:1500-1724,1728-2040,2043-2085` | Retain successful sessions/discovery/elicitation; normalize every refusal, including accepted-response alternatives and empty resume behavior |
| Body read, deserialize, unexpected variant, internal error | rmcp `transport/common/server_side_http.rs:181-270` | Replace dynamic decoder/body error output and logs, install bounded reader before collect, distinguish parse from envelope failure |
| SSE/session workers | rmcp `tower.rs:1134-1192,1319-1327,1393,1887-1893` and session dispatch | Fixed events before persistence and tracing; never persist raw refused envelopes or dynamic internal failures |
| AW1 stdio | `docs/AW1-HEADLESS-KINEWRIGHT.md` §4; rmcp `transport/async_rw.rs:143,172` | Same shape/refusal code before transport-io codec. Replace read/parse logging before formatting, bounded newline framing; no error-to-notification fallback |
| rmcp peer admission/results | rmcp `service.rs:1340-1342,1438-1468,1535-1549,1581-1597,1694-1715,1741-1763` | Fixed events before logging peer info, request/notification/result/client error, cancellation reason, rejected peer requests or shutdown errors. Route peer-admission protocol errors through R4 before sending |

The initialize mismatch at `tower.rs:479-487` currently traces both header and body before returning a formatted error.
Remove those fields at that producer, not in a later response filter. Apply the same rule to Host/Origin warnings,
body decoder errors, SSE trace messages, URI/session-store errors and panic payloads. Log only fixed event enum,
transport, phase, numeric bounded byte counts and a locally generated event number. Neither ids nor request-derived
headers, methods, metadata, params, session tokens, paths, raw errors or client error text enter tracing/incident
records. Sanitize before span creation and log formatting at every level, including trace. A logging filter is not the
security mechanism. Known event text is the literal T token plus newline on stderr; no dynamic suffix.

Refusal JSON plus stdio newline or its complete SSE event is at most 1,024 bytes; all HTTP refusal headers plus body
are at most 4,096 bytes, excluding TCP/TLS/chunk framing. Serialize into a bounded buffer before output. The emergency
no-id internal error is a prebuilt constant that also respects the bounds. Do not serialize an unbounded diagnostic and
then truncate it. The supported version/capability constants must fit this bound during release certification.

The listener admits at most 32 live HTTP connections, each with bounded head storage; excess connections receive the
prebuilt 503/transport_connection_limit and close. Use a proposed 30 s HTTP keep-alive idle timeout; no such timer applies to idle stdio.
The 8 MiB entity limit excludes transfer framing and the stdio newline; each escaped string and token is streamed under
that cap. Root object depth is 1; max object/array container depth is 64. Head size includes request line through final
CRLFCRLF. Header accumulation starts at the first byte of a new request, with a 5 s absolute deadline. Body time starts
when the complete admitted head acquires a reader permit, even if no body byte follows; the 5 s deadline never resets.
For stdio, start the 5 s frame timer at its first byte, including leading whitespace, and stop at newline. Idle stdio
between frames has no read deadline and holds no body-reader permit. Clean idle EOF is silent; EOF inside a frame is a
parse refusal. Four reader permits are global to an agent instance across transports. Reject the fifth without polling
its HTTP body; stdio may consume only its first frame byte before the same check. Acquire/release through all error and
cancel paths; cap retained framing buffers as well as logical lengths. Reject compressed HTTP entities rather than
creating an unbounded decompressor. Parser/tracing patches are release-pinned and costed in §11.

### 0.4 Part 0 witnesses, raw wire and controlled readers

G0a uses raw TCP HTTP/1 bytes and actual stdio pipes against the real endpoint on Windows and Linux. Send integer ids
-9223372036854775809, -9223372036854775808, -9223372036854775807, 9223372036854775806, 9223372036854775807 and
9223372036854775808. Legal endpoints dispatch; the two overflow values get exactly -32600. Include 1.0, 1e0, null,
bool, array/object and id-bearing notifications/initialized. Exercise canonical string encodings of 255/256/257 bytes,
ASCII, escaped controls/quotes/backslashes and multibyte Unicode; exact byte length is part of each fixture. Duplicate
and escaped duplicate id/method/result/error/_meta keys refuse. Request+result, method+error and result+error refuse
with no dispatch or correlation. Valid request, notification, result and client error are independent controls.

G0b sends every R4 refusal with unique canaries in URI, Host/Origin, auth, headers, _meta, method, id, params, session
id, body-read errors and internal diagnostics. The admitted id may appear only in its canonical correlation member,
never logs. Capture raw JSON, HTTP head/body, SSE framing and all tracing/incident output at trace level. Compare to the
literal constructors, statuses, error codes, connection EOF and byte caps. Include initialize mismatch at the producer,
modern declarations on a legacy connection, header/body conflicts, missing fields, unsupported versions, GET/DELETE,
resume and pre-parser failures. Assert zero revision/undo/dispatch change for each refusal.

G0b-version-precedence uses this exact otherwise structurally admitted body, with valid authentication, route,
content/accept headers and every method mirror required by the certified profile. Deliberately omit only
MCP-Protocol-Version:

```json
{"jsonrpc":"2.0","id":17,"method":"server/discover","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":7,"io.modelcontextprotocol/clientCapabilities":{}}}}
```

The HTTP result is 400, with exactly
`{"jsonrpc":"2.0","id":17,"error":{"code":-32020,"message":"Header mismatch","data":{"code":"transport_header_mismatch"}}}`,
R4's fixed headers and connection close. It is not -32602. Add only MCP-Protocol-Version: 2026-07-28 and the same body
gets 400/-32602/transport_invalid_params. The same stdio body gets -32602 plus one LF and continues. Discover selects
modern syntax without making the malformed value valid. Also cover _meta:7, absent body version, a non-discover request
on an established legacy peer, and initialize's optional absent header versus malformed present header. A corrected
modern string with matching mirrors must discover successfully. Every refusal has zero dispatch/revision/undo change.

G0c pairs raw-wire integration with a barrier-controlled reader and test clock at the real acquisition seam.
Exercise container depth 63/64/65, entity sizes 8,388,607/8,388,608/8,388,609, and heads
16,383/16,384/16,385 bytes. Make otherwise valid messages of the exact sizes, not malformed padding. Include fixed
length, chunk-crossing, omitted length, invalid length/chunks/trailers, duplicate/conflicting length, 128/129 fields,
never-ending frames and a reset. Hold four readers at barriers; the fifth must be rejected before its body poll count
changes. At 4,999 ms acquisition remains pending, at 5,000 ms it refuses, and a delivered completion at 4,999 ms succeeds.
Assert retained capacities and bytes under caps, all permits returned and the exact disposition. Hold 32 admitted
connections and assert the 33rd receives the fixed connection-limit response without an allocated request body. Leave stdio idle for
more than 5 s between two valid calls; both succeed. Clock-controlled checks supplement raw-wire checks; neither alone
establishes all parser and acquisition paths.

G0d is the successful-elicitation control on both HTTP and stdio using AW1's real consent path. Negotiate legacy form
elicitation, withhold the unconditional allow flag, request an actual destructive edit, observe a registered outbound
elicitation id, reply with its valid accepted result and assert one snapshot and one commit. Decline/cancel prevents
commit. In another call send a valid correlated client error, e.g. -32601 with fixed test text; assert HTTP 202/empty
or no stdio output, settlement as failed consent, no commit, and a later ordinary request succeeds. An id-less modern
peer error is accepted/discarded without settling anything; id:null is a separate -32600 case. Repeat a modern
input-required/complete round through rmcp's request-scoped flow with declared capabilities. No missing-method rejection
is allowed for the legacy result/error controls. Other controls cover roots/sampling replies when advertised.

G0e tests the documented -32022 compatibility deviation on HTTP and stdio. Send an admitted request id 18 with a
well-typed unsupported body version, matching HTTP mirrors where applicable, and otherwise valid modern metadata.
Use a unique canary in the version, compare exact R4 bytes with the literal supported array and absent requested, and
assert no canary in wire/log/incident output. Repeat with a second version of different length; the refusal bytes must
be identical except for an intentionally changed admitted correlation id. Neither request dispatches.
One client fixture accepts the generic JSON-RPC error, correlates id 18, and reports unsupported-version failure using
only code/supported. A strict fixture validating UnsupportedProtocolVersionError must reject the absent requested and
surface one bounded local protocol-compatibility failure. Neither client treats it as success, commits consent,
automatically downgrades, loops retries or logs raw payloads. A fresh, explicitly supported-version request succeeds.
Record the strict rejection as an expected compatibility limitation, never a specialized-schema conformance pass.

Runtime red for G0 uses today's malformed-id and canary-mismatch paths; a compile/setup failure is not a red witness.
After green, remove individually the id precheck, response correlation, version precedence, pre-log sanitization,
reader/depth caps, deadline start and refusal egress bound. Each targeted witness must fail for its named invariant.
Both OS lanes must pass G0a-G0e before either new clip kind is exposed.

## 1. Scope and current foundation

AW0 A1 withdraws native Remotion parity, keeps SVG first, requires external person-installed runtimes and makes Lottie
later import work (`docs/AW0-AGENT-WORKBENCH.md:43-79`). The slice order is AW1, AW2, AW3, AW4; AW3 is not an AW2
prerequisite (`docs/ROADMAP-AND-WORKFLOWS.md:1038-1046`). AW1 specifies shared project IO, authenticated HTTP/stdio,
lazy headless rendering and live proxy edits (`docs/AW1-HEADLESS-KINEWRIGHT.md` §§2/4/4a/6). Those are prerequisites,
not an assertion that every AW1 stage is implemented. Current server startup still binds HTTP inside the constructor
(`crates/kinewright-agent/src/server.rs:817-868,965-987`).

The current five clip kinds are Media, Title, Freeze, Adjustment and Solid; span versus asset helpers already
distinguish their mapping rules (`crates/kinewright-core/src/model.rs:335-425`). Curves use integer values and
clip-local keys; the proven value bound is ±1,000,000 and key positions are bounded to ±2^40
(`crates/kinewright-core/src/automation.rs:9-74`). The existing rebase function visits effect and enable owners, not
code params (`crates/kinewright-core/src/operation.rs:3074-3100`). AW2 must add its owner.

R6 [P1/P2]. Ship native SVG and optional external Remotion clips on one typed parameter vocabulary. No raster
image-model integration, JS expressions in Core, artboard/pen tools, Lottie authoring, or native React/layout runtime.
Both new kinds are silent video-track clips, use checked span arithmetic, ordinary effects/blend/transitions/enable, and
reject audio and speed setters. External frame selection follows the finite mapping in R20; it must not restart on split or
head trim. Keep Media and Freeze asset semantics unchanged.

## 2. Part 1: document, descriptors and SVG bindings

R7 [P1, stored shape]. Add `ClipContent::Code(CodeClip)`. Its language is a closed versioned tag, initially `svg` API 1.
A CodeClip references an immutable CodeAsset by content hash and carries a sorted map of typed parameter values plus a
sorted numeric-param-to-`AutomationCurve` map. The document's new `code_assets` table contains source hash/length,
language/API, declarations, bindings, font hashes and bounded provenance. Empty tables serialize away. Raw SVG never
appears in `Operation`, journal entries or the served schema.

R8 [P1, declared parameters]. A declaration has a stable name, type, default, optional numeric min/max/unit/scale,
animation flag and a short description. Use a separate table, never `EFFECT_DESCRIPTORS`, as required by AW0 §5
(`docs/AW0-AGENT-WORKBENCH.md:314-320`). Core validates complete descriptor/value agreement, unknown names, duplicates,
required values, ranges and curve types. Descriptor identity is hashed with bindings; source replacement is one atomic
revision, with an explicit complete parameter replacement. No best-effort carryover of old curves against changed
declarations.

| Type | Stored value | SVG target | Animation in AW2 |
| --- | --- | --- | --- |
| Integer/fixed decimal | i64 and declared power-of-ten scale, at most 10,000 | Geometry, opacity, font size, typed transform component | `AutomationCurve`; raw bounds inside ±1,000,000 |
| Boolean | bool | Declared visibility switch | Constant |
| Text | UTF-8 string | Text node content only | Constant |
| Colour | Straight RGBA8 | Fill/stroke colour only | Constant |
| Enum | Declared string token | Enumerated safe presentation value | Constant |

R9 [P1, bindings]. Bind a parameter to an exact, unique SVG id and an allowlisted target field. Validate target element
and field compatibility before commit and again at render preparation. Missing or duplicate ids, unbound declarations,
competing writers to one field, unknown attributes, incompatible types and reference cycles fail closed. A text binding
replaces text content; it cannot write XML, CSS, a URL or a path. Transforms bind named translate/rotate/scale
components with a fixed composition order. No arbitrary attribute names, XPath, selectors, script or source-string
interpolation.

Compile the validated XML into an immutable template with indexed binding sites. Instantiate typed values into its nodes
and serialize through an XML writer before usvg parsing. Escaping text is serialization, not textual templating. Do not
assume resvg exposes mutable text/geometry setters. Cache the usvg tree only for an identical evaluated-parameter key.
Measure that parse cost in the per-frame gate; do not omit it to claim live scrubbing.

R10 [P1, animation]. Evaluate numeric params at project frame minus clip start using the existing integer curve
evaluator. A curve replaces the parked value. Extend the existing keep-outside rebase owner walk: head trim shifts keys;
split preserves the full curve on each half with the right origin shifted; roll/slide rebase only touched heads by
their signed head movement; the slide middle only moves. Move/ripple preserve local keys. Validate ordered-only, including negative keys and the existing position/value proof bounds. This follows MO1
§§3/R11-R14, not the audio destructive-trim policy (`docs/MO1-KEYFRAMES-TRANSFORM-STILLS.md:202-268`). Also cover copy
attributes, undo/redo, replay and serialization; copying code params requires matching CodeAsset identity. Source itself
has no curve, clock or implicit animation.

## 3. Part 1: blob ownership, migration and provenance

R11 [P1, store]. Use the project-derived `<stem>.kinewright-assets` root, with separate `code`, `fonts` and `renders`
directories. Names are validated SHA-256 hashes plus fixed suffixes. The LUT precedent derives its root at runtime and
uses hash-only filenames (`crates/kinewright-media/src/lut_store.rs:1-42`). Extend that discipline; do not use the
project-file FNV digest as an asset hash. Import performs bounded read, strict validation, hash verification and
temp-write/sync/atomic publish before an Operation can reference the asset. Verify length/hash when resolving stored
assets, with immutable opened handles through use. Reject symlinks/reparse points and root escapes, including swaps.
Core validates records without IO; the project/media boundary validates bytes. That boundary validates imports and every
public GUI/agent/headless commit, load and replay before applying an asset-referencing operation. It holds the verified
blob handles through commit admission; hashes bind the descriptors and target inventory to those bytes. Core owns
structural parameter/curve validation, not filesystem availability. Render entry still checks integrity.

Referenced source, fonts and completed external frames are durable project assets. Cache inventory accounts for their
bytes and pin status. Ordinary M41 clear/eviction can remove only unreferenced or reproducible scratch entries; it never
removes the sole media needed by a saved project or retained undo. Count pins from the live document, saved checkpoint,
retained undo/redo, branches, recovery journal and in-flight render/export. Refuse admission when pinned bytes fill the
cap. No unbounded secondary history or reference map. Crash leftovers remain unreferenced and are reclaimed by scoped
inspection.

R12 [P1, Save As]. Copy and verify every required code/font/render blob to the destination before atomically publishing
the new project. A failed copy leaves the original and prior destination intact. Unsaved projects use a session-owned
temporary store; first save follows the same copy-first rule. This is a deliberate stronger contract for AW2 assets: the
current LUT save path writes project bytes before copying and reports copy failures afterward
(`crates/kinewright-project/src/project.rs:520-549`). Do not silently inherit that ordering or rewrite unrelated LUT
behaviour in this slice.

R13 [P1, format]. Raise maximum supported format from 2 to 3. Current max is 2; the shared writer computes
minimum-required version and skips v1's envelope key (`crates/kinewright-core/src/model.rs:1254-1263`;
`crates/kinewright-project/src/project.rs:218-256,372-412`). Extend that one predicate: any Code/External clip or
nonempty AW2 asset/spec table requires 3, including disabled clips and parked/undo-restored content in the saved
document. Otherwise retain the existing v1/v2 predicate verbatim. Do not change journal envelope version; writer maximum
becomes 3, as MO2 R7 requires for future ops (`docs/MO2-BLEND-ADJUSTMENT-TRANSITIONS-SOLIDS.md:749-774`).

Migration is additive defaults, no eager write and no source conversion. Old v1/v2 fixture saves must remain
byte-identical. V3 round-trips the exact source references, descriptors, params, curves and render references. Unknown
language, API, binding or adapter versions refuse with a typed error, even if disabled. Test the actual prior reader:
unknown new clip kinds can fail parsing; an otherwise parseable newer envelope must refuse overwrite. Do not claim old
readers can render Code or preserve unknown fields through Save As.

R14 [P1/P2, provenance]. Store declared origin separately from verified facts. The bounded record includes source/spec
hash, descriptor hash, font/dependency hashes, renderer/API/build identity and optional authored-by attribution. Render
reports add project revision, clip/frame, evaluated-param hash, raster hash, colour/alpha contract, timings and budget
outcome. External reports add executable/version/browser identity, normalized command template, lockfile and input
manifest. No bearer tokens, ambient environment, source text or props in logs. User-written author/model claims are
labelled declared, never verified.

## 4. Part 1: renderer, purity and resource bounds

R15 [P1, dependency]. Choose exact `resvg = 0.45.1`, with its usvg parser and tiny-skia rasterizer. These versions
already occur in the lock (`Cargo.lock:3711-3723,4477-4489,4777-4797`); the study recommends resvg
(`target/review/refs/synthesis.md:29-32`). Add text support deliberately. Disable default features on the new
dependency; load no system fonts or font mmaps and admit no raster images. Audit feature unification through egui:
disabled defaults alone do not remove features another consumer enables.

Upstream describes reproducible static rendering without system libraries and licenses this release MIT OR Apache-2.0.
Select MIT and carry notices; the transitive dependency/font inventory remains an exit artifact. This makes it a
suitable candidate, not measured proof of our cross-OS bytes or RSS. See [the pinned
README](https://github.com/linebender/resvg/blob/v0.45.1/README.md) and
[manifest](https://github.com/linebender/resvg/blob/v0.45.1/crates/resvg/Cargo.toml). No browser runtime or native font
engine is required by the proposed path. Build the native helper for the common x86-64 CPU baseline on both OSes,
without `target-cpu=native` or fast-math changes. Any renderer/build-profile upgrade requires G1 again; upstream
reproducibility is not a waiver for that gate.

R16 [P1, SVG profile]. API 1 accepts shapes/paths, groups, gradients, local clip paths and text/tspan with explicit
numeric presentation attributes. Reject DTD/entities, scripts/events, foreignObject, image/data URLs, href/use,
filters/masks, animation, embedded stylesheets, external CSS and all nonlocal references. Validate every feature before
usvg; unsupported features cannot silently disappear. Resource resolvers always refuse file/network/data reads. All
geometry is finite and bounded; all local reference graphs are acyclic. This small documented profile serves authored
graphics, not general SVG import.

R17 [P1, fonts]. Reuse the embedded Inter asset exposed by `title_font_bytes`
(`crates/kinewright-core/src/title.rs:8,316-318`). Rev1 records 876,576 bytes and SHA-256
`29160a80ff49ddcab2c97711247e08b1fab27a484a329ce8b813d820dc559031` (rev1 §4 R17; reverify at P1 entry). Pin those bytes in
renderer identity and project font records. Explicit font selection and glyph coverage are required; missing glyphs fail
rather than select a machine font. Extra fonts are project-owned hash blobs with recorded licences. Upstream Inter is
OFL-1.1; carry the font notice and verify the exact bundled file's provenance before release ([Inter
licence](https://raw.githubusercontent.com/rsms/inter/master/LICENSE.txt)).

R18 [P1, hard limits, adopted Q2]. Keep the raster function pure over already-loaded bytes/params/dimensions. Put it in a small
native Rust helper, with no FFmpeg, GPU or UI initialization, to make timeout/OOM recoverable. The independent R28 guardian owns the helper before it executes. One lazily started worker
serves an active preview/render session, one frame at a time; it exits when that session finishes, cancels, fails or
becomes idle for 2 seconds. No permanent worker pool and no process spawn inside the raster function. Enforce a 512 MiB
helper allocation/OS resource ceiling, bounded IPC and a parent watchdog. Kill and reap on deadline; discard late or
partial replies. Linux uses its owned cgroup memory.max plus an address-space limit, and Windows a Job memory limit; report those as different metrics,
not as interchangeable RSS. Helper RSS counts as native Kinewright memory in every budget, never as an exempt external
renderer.

The pinned `resvg::render` API takes a tree and pixmap and exposes no deadline or cancellation argument ([versioned
API](https://docs.rs/resvg/0.45.1/resvg/fn.render.html)). Therefore a dropped `spawn_blocking` future cannot establish a
hard time bound, inferred. The lead adopts the native Rust helper as the explicit exception to AW0 §8's no-platform-sandbox-code cost assumption. This is
resource isolation, not a claim of hostile-code sandboxing. Do not proceed with an unkillable thread while calling the
gate satisfied.

R19 [P1, pixels]. Resvg produces premultiplied display-sRGB RGBA8 through tiny-skia ([pixmap
contract](https://docs.rs/tiny-skia/0.11.4/tiny_skia/struct.Pixmap.html)). Pin unpremultiply per channel to zero at alpha a=0, otherwise
`min(255, floor((255*c + floor(a/2))/a))` for premultiplied channel c, with alpha unchanged. Then use the existing
straight display-pixel-to-working conversion. Titles/solids already use
`WorkingFrame::from_display_frame` and carry clip identity/effects/blend through `DecodedLayer`
(`crates/kinewright-media/src/render.rs:61-90,497-592`; `crates/kinewright-media/src/frame.rs:101-119`). Add Code and
External pixel sources through that seam, preserving project frame, z-order and transition. No private inverse-sRGB or
direct premultiplied upload. Re-gate any CC8 colour context conversion that lands before implementation; include context
in working-pixel cache identity. MO2 assigns Code/blend interplay to AW2
(`docs/MO2-BLEND-ADJUSTMENT-TRANSITIONS-SOLIDS.md:1194-1209,1312`).

## 5. Part 2: external clips and the Remotion adapter

R20 [P2, finite frame mapping]. Add `ClipContent::External(ExternalClip)`, referencing a versioned immutable
ExternalSpec. The spec contains source/input snapshot, entry/composition id, descriptor hash, typed constant props,
composition length N, dimensions, rational rate, dependency identity and render key. N is positive, at most 1,000 in
v1. Composition frame indices are [0,N). V1 rate equals project rate exactly as reduced rationals, speed is 100,
output is silent display-sRGB and reverse/retime setters refuse.

There is one composition origin O in ExternalClip, an integer with 0 <= O < N. `Clip.source_range = [a,b)` remains a
nonnegative span coordinate, with a < b. It is not a second unrelated media frame range. For timeline start T and
integer project frame p, define D = b-a and the selection equation:

`visible(p) iff T <= p < T+D; u(p) = a+(p-T); frame(p) = O+u(p)`.

The invariant is `0 <= O+a < O+b <= N`; all additions/subtractions are checked i64 arithmetic, and T+D must be
representable. No clamping, looping, duplicate-last-frame or implicit shorter-source truncation is permitted.
O is fixed across ordinary edits and makes an imported subcomposition offset explicit; defaults are O=0, a=0, b=N.
Stored full-composition frames [0,N) make extensions within bounds possible without re-render. Effects, transitions and
enable curves continue in clip-local project time p-T, independently of frame selection.

| Edit, with delta positive toward later timeline time | Exact External action |
| --- | --- |
| Head trim by delta | T'=T+delta, a'=a+delta, b'=b, O'=O. Extension uses negative delta and must satisfy a'>=0 and the finite invariant. Rebase effect/enable keys by -delta |
| Tail trim by delta | b'=b+delta; T,a,O unchanged. Negative shortens. Refuse beyond N or an empty span |
| Split at T+k, 0<k<D | Left [a,a+k), start T; right [a+k,b), start T+k; both retain O/spec/key. At the split the right starts at O+a+k, never zero. Right-side effect/enable keys shift by -k |
| Roll shared cut by delta | Left tail bL'=bL+delta; right head TR'=TR+delta, aR'=aR+delta. Validate both clips atomically, including media neighbors' own rules. Refuse the entire edit if either finite bound fails |
| Slide clip by delta | Middle T'=T+delta, same O,a,b and source frames. Apply a left-neighbor tail trim +delta and right-neighbor head trim +delta. Validate all three atomically; only a neighbor whose head moves rebases its local curve origin |
| Move / ripple move | Change T only, preserve O,a,b and local curve keys; validate timeline arithmetic and normal collision rules |
| Source/spec or prop replacement | One atomic SetExternalSource/parameter revision keeps O,a,b and timeline duration unless an explicit complete replacement mapping accompanies it. Require O+b<=Nnew, equal rate and valid params. A shorter source that cannot cover the retained range refuses external_source_range; no automatic trim. ReplaceClip/FitToFill remain Media-only |

The current generated-span validator checks only nonnegative increasing endpoints
(`crates/kinewright-core/src/operation.rs:5037-5044`); External must use a new finite validator in every edit arm and
whole-document validation. Validate at prepare/commit before revision/undo change, load before publication, and each
journal replay operation before it applies. Replay errors stop at the last valid revision and preserve the journal.
Also validate before render as corruption defense. Do not validate solely in an external-render adapter. Undo/redo
restores the exact O,a,b,T/spec tuple. A rate-changing document operation that would violate an existing External spec
refuses atomically until an explicit compatible spec/mapping replacement is supplied.

G2-External uses eight distinguishable raw numbered/color frames 0..7, N=8, O=1, [a,b)=[1,6), T=100.
Frames at p=100..104 must be 2,3,4,5,6, and p=105 must be absent. Head +2 gives 4,5,6; restoring the head returns
2..6. Tail +1 admits frame 7; tail +2 refuses with external_source_range. Split at 102 gives left 2,3 and right
4,5,6. Independent reset fixtures make roll/slide/move literal. For roll, left O=0,[0,3),T=100 and right
O=0,[3,6),T=103 become left [0,4),T=100 and right [4,6),T=104 on +1, with pixels 0..3 then 4,5.
For slide, neighbors O=0,[0,2),T=98 and O=0,[1,4),T=105 surround the original middle.
Slide +1 yields left [0,3),T=98, middle unchanged O=1,[1,6),T=101, right [2,4),T=106.
The middle still displays 2..6. A standalone move to T=200 preserves O=1,[1,6) and displays 2..6 at 200..204. Include a=0, O+a=0, O+b=N, b=a, negative origin, overflow, last frame N-1 and exclusive N.
Shorter replacement Nnew=6 refuses the retained O+b=7, while Nnew=7 succeeds. Test each illegal tuple via commit,
loaded JSON and replay with no revision/undo mutation. Undo/redo and save/replay reproduce every numbered result.
This gate is independent of Code parameter ownership and cannot pass by rejecting all External clips.

R21 [P2, person-controlled execution]. External execution defaults off. A local person grants it per canonical project
identity via a GUI action or an explicit headless launch setting, never a document field or MCP capability.
Opening/copying/downloading a project cannot grant permission. Save As/new project identity requires a grant. Revocation
cancels active/queued jobs; completed stored frames still work. The UI states that project code, including later
agent-authored changes in that project, runs with the person's privileges. Use AW1's authenticated owner/roots on proxy
and standalone paths; investigator sessions cannot execute renderers. This implements AW0 §5/§8's trust rule
(`docs/AW0-AGENT-WORKBENCH.md:365-369,511-526`).

Kinewright constrains its resolved inputs/output handles to allowed roots. It cannot restrict arbitrary trusted JS
file/network access with path checks. Do not describe this as an OS security sandbox. The adapter's supported input
contract is project-owned local data and assets; remote fetches, wall clock, unseeded randomness and undeclared
environment inputs are outside reproducible authoring. Record a seed when used. Stored verified frames remain
authoritative even when arbitrary external source cannot reproduce them elsewhere.

R22 [P2, installation/versioning]. Resolve an explicit person-configured Node binary, project-local Remotion packages,
and an existing compatible browser. Spawn exact executables with argument arrays, never a shell, npx, npm install or
package scripts. Probe versions only after permission, under the supervisor. Missing Node/Remotion/browser gives
distinct fixed typed errors and manual installation guidance. Never download a browser implicitly. The public API
otherwise permits a browser download; explicitly supply `browserExecutable` ([Remotion browser
option](https://www.remotion.dev/docs/renderer/render-frames#browserexecutable)). No Node, Python, Chromium or Remotion
package ships in Kinewright.

Initial adapter protocol 1 targets Remotion 4, using APIs available at 4.0.515; support only exact certified
Node/Remotion/Zod/browser tuples from a small allowlist, not all of 4.x. P2's first task pins the maintained patches and
package hashes on both OSes. Outside the allowlist, fail `external_renderer_version_unsupported`. A changed tuple
requires explicit spec refresh and a new key; it never silently overwrites a prior render.

Remotion has its own licence rather than an OSI licence (AW0 A1:76-79). The adapter is Kinewright-owned glue using the
person's installation, with no copied renderer code. Release docs link the applicable [upstream
licence](https://github.com/remotion-dev/remotion/blob/main/LICENSE.md) without hardcoding a price or eligibility
threshold. No licensing service or account requirement is added to native SVG or cached playback.

## 6. Part 2: schema mapping and cache identity

R23 [P2, explicit schema bridge]. Require a project adapter manifest naming the composition entry and an importable
schema module. That module exports the same Zod schema/defaults used by the composition. A short Kinewright-owned Node
bridge performs supervised describe/validate/render commands; it is source glue, not a bundled runtime. Importing this
module executes code and therefore also requires R21. Do not claim automatic schema discovery from `getCompositions`;
its documented result is composition configuration ([API](https://www.remotion.dev/docs/renderer/get-compositions)).
Remotion attaches Zod through a Composition's schema prop ([schema contract](https://www.remotion.dev/docs/schemas)).

R23a [P2, constraint authority]. The version-pinned visitor derives the native descriptor directly from each
inspected `_zod.def`, its node type, object catchall/strictness and every check's definition. That traversal is a
published Zod Core extension contract ([Zod Core](https://zod.dev/packages/core)); only the certified exact Zod build is
supported. Recognize required strict objects with finite scalar properties and bounded nested objects. Flatten paths
by reversible JSON pointers. All required values come from a separately exported bounded literal default object.
Integers, bounded fixed decimals, boolean, bounded text and string enums map to R8. Every nested object must reject
unknown keys. Reject optional/nullable/default wrappers, records, arrays, unions, coercion, transforms, preprocessors,
custom refinements, format/regex checks and every unknown node/check. Reject rather than approximate a constraint.

Accept numeric finite lower/upper bounds with their inclusive flags, integer checks and an exact fixed-decimal grid
whose scale is declared and independently validated. Convert exclusive bounds onto that integer grid with checked
arithmetic. Native raw values remain inside ±1,000,000, and the actual JS value must round-trip through the certified
bridge number codec without changing the agreed value. Zod numeric multiples or float checks unsupported by that exact
codec refuse. Both the actual Zod parse and the native descriptor must accept literal defaults before committing a spec.

String min/max/length checks use JavaScript UTF-16 code units for the certified Zod build. Store this length unit in the
descriptor; Core counts BMP scalars as one and supplementary scalars as two. Reject unpaired surrogates at import;
valid text is Unicode with no normalization. Independently enforce R8's UTF-8 byte cap. Test ASCII, combining text,
precomposed characters and an emoji at max 1/2, rather than substituting Rust byte or scalar length for JS length.

R23b [P2, metadata]. JSON Schema conversion is optional exported documentation, never validation authority. If used,
call it with a new empty metadata registry, no override callback and unrepresentable forms treated as errors. Zod
explicitly permits metadata to override generated keywords ([JSON Schema metadata](https://zod.dev/json-schema#metadata)).
Copy only bounded title/description, unit and explicit color UI annotations in a separate pass. They cannot change
node type, numeric bounds, required keys, strictness, string-length units or scalar encoding. A color annotation selects
a UI presentation for an already constrained string enum or a natively validated color encoding; it cannot turn an
unconstrained string into trusted RGBA. A declared decimal unit/scale must pass the independent grid checks above.

Certification fixtures put contradictory metadata on type, minimum/maximum, required/additionalProperties, properties
and string length. Descriptor bytes and acceptance stay those of the inspected definitions. Core rejects an over-max
edit even if metadata loosens max; valid values still pass real Zod even if metadata claims another type. Invalid
imports/edits leave revision, document, prepared plan and undo unchanged. The bridge rechecks complete props with the
same actual schema just before rendering. A disagreement is external_schema_mismatch, no ready publication. No schema
module executes on cached open/play/proof/export; schema describe/validate is code execution subject to R21/R28.

V1 checks the resolved composition metadata before rendering: size, rate and frame range must match the frozen spec.
If `calculateMetadata` changes them, return `external_metadata_changed` and require an explicit spec refresh, not an
output silently rendered under the wrong key.

R24 [P2, source identity]. Use two immutable image sets per job: a changing source/input image and a reusable certified
runtime/dependency image. The latter includes the actual Node binary, browser executable and its companion files,
Remotion/Zod packages and the certified runtime support-file closure. A version string or lockfile alone is insufficient.
Record sorted normalized relative paths and SHA-256 hashes for entrypoint, schema/bridge exports, public assets, fonts,
package manifest, lockfile and all declared inputs. Reject escaping links, unresolved inputs and unsupported filesystem
aliases. Source snapshots are capped at 256 MiB/10,000 files; each streamed dependency/runtime manifest at
16 MiB/100,000 entries. Keep manifests as blobs and only hash/length references in document metadata. Never buffer the
full tree or copy node_modules into the project. Hash the actual admitted bytes, not a pre-copy directory listing.

Provisioning belongs to the person. They install Node, browser, project packages and the OS image utilities, then
prepare the images through a documented external setup workflow. Kinewright never bundles Node or Python, downloads a
browser, installs packages, runs package scripts or silently creates/mounts privileged images. Its native snapshot
handoff captures bounded source blobs into the existing owned store; person-run image tooling materializes those exact
blobs. The final mounted image must match the captured manifest before any JS or version probe runs. This is a snapshot
of those captured bytes, not a claim that a changing working tree was sampled at one atomic instant.

A source/asset edit captures a new manifest and requires a newly sealed source image. On Linux the person uses
mksquashfs on the captured files. On Windows they use person-run DiskPart to create/attach a new VHDX read/write,
populate its NTFS volume from the captured files, close all writers/mappings, detach, then attach read-only.
Updating an old mounted image in place is unsupported. Pending edits cannot render until the new image is
available and verified. Old jobs retain their old image identities; publication still checks the desired spec/key.
The reusable runtime image changes only on an explicit person-provisioned tuple refresh. Manual source-image refresh
is an explicit Part 2 setup/workflow burden; no automatic image builder or installer is hidden in this forecast.

The following are concrete supported mechanisms, subject to native certification rather than claimed current behavior:

| OS | Changing source/input snapshot | Node/browser/dependency identities through execution |
| --- | --- | --- |
| Linux | A distinct read-only SquashFS source image with fs-verity enabled on its backing file before admission. Verify the fs-verity digest and full SHA-256/manifest, and retain the opened backing inode plus mounted-root handles. SquashFS freezes directory entries as well as file bytes | Use the same protection for the reusable runtime image. The certified person-provisioned service profile gives R28's guardian an isolated mount namespace with pinned read-only bindings for both image roots. Node starts via execveat(AT_EMPTY_PATH) on its verified binary fd; the explicit browserExecutable and all child imports resolve inside those same fixed image bindings. Retain binary/root/backing handles until tree cleanup |
| Windows | A distinct read-only attached NTFS VHDX source image. Before admission acquire backing-file read handles with FILE_SHARE_READ only, denying write/delete sharing, and retain them. Verify the read-only attachment, backing file id/volume and mounted volume identity, then stream-check every captured file | The same lease covers the reusable runtime VHDX. Open Node/browser with CreateFileW(GENERIC_READ, FILE_SHARE_READ, OPEN_EXISTING), retain their file ids/handles and immutable volume-root identities. CreateProcessW's explicit application path and browserExecutable use paths on those verified volumes, never PATH, a drive-letter alias or the original installation. Verify the suspended Node process image before resume; browser launches use the same stable image namespace |

[SquashFS](https://www.kernel.org/doc/html/latest/filesystems/squashfs.html) supplies the read-only directory tree.
[fs-verity](https://docs.kernel.org/filesystems/fsverity.html) supplies backing-file byte protection, not pathname
protection. The person-approved native service setup uses unshare(CLONE_NEWNS), recursively private mount propagation
with MS_PRIVATE|MS_REC, and read-only bind mounts of the admitted image roots. Children inherit that namespace. Verify
the actual bound roots and backing inodes before hashing/COMMIT; reject if namespace privileges/setup are unavailable.
These bindings pin paths even if a host backing pathname or public mount alias is replaced. They bind already mounted
person-prepared images; Kinewright does not attach a new filesystem image. Record mount/device/inode identities.
Runtime support files must come from the frozen image or explicitly certified stable OS roots; writable search paths, NODE_PATH/NODE_OPTIONS
overrides and browser auto-update paths cannot select different binaries/modules during a job.

On Windows the [read-only attach flag](https://learn.microsoft.com/en-us/windows/win32/api/virtdisk/ne-virtdisk-attach_virtual_disk_flag)
protects the mounted filesystem; [CreateFile sharing](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew)
protects the backing files and executables from write/delete opens. The certified provisioning handoff must also
exclude pre-existing writable mappings, alternate writable attachments and differencing parents. Keep mount-point
ancestors against rename/reparse replacement, use verified volume-GUID roots and hold the attachment lease until
cleanup. A read-only file attribute alone is insufficient. Certify that retained leases coexist with the supported
Node/browser/virtual-disk opens; a sharing conflict, detachable/unprotected root or unavailable privilege refuses
external_input_stability_unavailable before execution. The person supplies any required OS privilege/setup.

One native image-lease owner serves probe, describe, bundle and render, including browser subprocesses. It acquires
protection before hashing, checks actual opened identities before the R28 COMMIT barrier, and retains root/backing/
executable handles until OS emptiness and cleanup verification. Ownership survives guardian failure through the app's
cleanup owner; after app death the guardian retains it. These are input leases, never extra Windows lifetime Job handles.
If both owners fail, OS containment closes the tree; restart recovery retains store pins and refuses new execution
until it verifies retirement. Writable caches/temp/output use separate staging roots and cannot shadow admitted inputs.
Final manifest/hash/identity checks are additional diagnostics, not the stability mechanism. Ordinary mutable trees,
chmod, advisory locks and pre/post hashes alone remain unsupported. This is the trusted R21 profile, not protection
against an administrator replacing devices or trusted code deliberately escaping the declared input contract.

G5-stability first completes a real describe/render on each certified OS profile. Then pause the production job at
barriers after identity acquisition/before exec, after Node start/before browser start, and after an initial input read.
Use distinguishable A/B source modules, assets, Node binaries and browser executables with recorded hashes/markers.
Exercise each identity separately:

- Replace the working source, original installations, backing-image pathname and public mount/drive aliases. The job
  must keep reading/executing admitted A from pinned identities, or refuse before a new user-code marker/publication.
  B must never run or contribute bytes under A's key. A deliberately refreshed B image must produce a new key and B.
- Attempt write-then-restore of source/assets inside the mounted snapshot, package files, Node/browser bytes and image
  backing bytes through alternate paths while paused. The OS must deny writes or preserve access to the original A
  image; collect the write result and independently observed file/process identities. Restoring bytes before the final
  hash cannot make execution of B under A pass. Also change-then-restore the mutable originals, which must not affect A.
- Hold a writer/writable mapping or provide a writable alternate attachment before acquisition. The profile must refuse
  admission, not proceed and rely on its final hash. Repeat lost-lease/guardian-failure cases with charges and pins held.

Use production image acquisition and launch paths, then separately mutate the source seal, namespace pin, executable
lease and backing protection. The corresponding replacement/change-then-restore witness must fail while a valid A
control still executes. Compare process image and numbered/alpha output identities, not only final file hashes or exit
status. P2c remains unaccepted until these real Windows/Linux witnesses pass; none was run for this revision.

R25 [P2, cache key]. Domain-separate and length-prefix the hash fields. Include adapter protocol/build, complete
snapshot/input/schema hashes, lockfile and dependency manifest, composition id, sorted typed canonical props, seed,
half-open frame range, dimensions/rational fps, colour/alpha contract, Node, Remotion and browser version/hash,
platform/render profile and effective flags. Native SVG keys instead hash source/declarations/bindings/fonts, evaluated
params, dimensions and exact renderer/build/profile. Do not use map iteration order, mutable paths, timestamps or
attribution as pixel identity. These extend the parent key requirements (`docs/AW0-AGENT-WORKBENCH.md:339-359,532-552`).

R26 [P2, durable output]. Choose straight-alpha RGBA8 PNG sequences, no ProRes transcode. The bridge calls
`renderFrames` with PNG, concurrency 1 and explicit browser, cache limits and cancel signal. It uses `outputDir: null`
and an awaited `onFrameBuffer` acknowledgement so the Rust owner can bound writes. These are documented API behaviours,
including backpressure from 4.0.515 ([renderFrames](https://www.remotion.dev/docs/renderer/render-frames)). Treat output
as display-sRGB with an explicit assumption; validate PNG dimensions, decoded length, alpha and colour chunks before
use. Reject contradictory tags. Decode one frame at a time; PNG sequences need an explicit reader in this slice, since
MO2 left image sequences deferred (§14). The bridge protocol uses bounded binary chunks and acknowledgements on stdout;
control records and stderr logs are separate. A frame up to 40 MiB crosses as chunks of at most 1 MiB, never as one
JSON/base64 message or an unbounded pipe buffer. One acknowledged chunk is in flight. Invalid framing fails the job.

Write to an owned staging directory. Validate every expected frame, reject missing/duplicate/out-of-range files and
unsafe entries, compute raw pixel and file hashes, then atomically publish a complete bounded manifest under the render
key. Publish only after process-tree cleanup succeeds. No ready marker or final frame from a partial render. Cold
playback verifies one opened frame at a time, with a bounded verification cache and export pin; it does not load the
entire sequence. Stored bytes carry the result across OSes; arbitrary Remotion re-renders are not promised cross-OS
pixel identity (AW0 §9).

## 7. Part 2: state, undo and process lifetime

R27 [P2, state]. Derived job states are disabled, missing-runtime, pending, rendering, verifying, cleaning, cleanup-blocked,
ready, failed or cancelled. Cleaning/cleanup-blocked are nonterminal and retain the independent cleanup owner. A source/prop edit is one ordinary revision/undo entry and changes the desired render key. Completion
publishes immutable assets keyed to that intent, without a second document mutation. Never attach output to "latest
revision" by relabelling it. Use project/session generation, clip/spec key and job identity; a late result may remain an
unreferenced blob but cannot satisfy another spec. Undo can restore the prior ready key. Dedupe matching work, globally
run one external job, and bound/replace queued obsolete edits rather than spawning on every slider event.

The GUI may show the old image with a visible stale marker while pending. Agent proofs return typed
`external_render_pending` and no misleading final image; final export refuses pending/failed/missing/changed frames.
Progress returns bounded status/hash references, never inline frames. Opening, playing, proofing or exporting a ready
saved project does not locate Node, import JS, probe versions or start a renderer. Editing pixels requires the renderer;
editing ordinary clip effects/placement over complete stored frames does not. This is the parent stale-output contract
(AW0 §5:360-364).

R28 [P1/P2, independent cleanup owner]. The app's RAII job manager owns intent and waits for cleanup; it is not the
parent-death guarantee. The native `kinewright-job-guardian` is a separate, bounded process, without FFmpeg/GPU/UI,
that survives abrupt app exit and owns every native SVG helper and external probe/describe/bundle/render process.
No execution occurs before this ownership exists. The existing direct-child probe utility has unbounded pipe Vecs and
only kills its direct child (`crates/kinewright-agent/src/child_process.rs:78-121`); do not use it for AW2 jobs.
Drain pipes continuously into 64 KiB rings, with bounded protocol chunks and backpressure outside UI/Core locks.

On Linux the guardian is the main process of a transient systemd user service with delegated cgroup-v2 control,
separate from the application's unit/scope. Its service owns a job subtree; guardian/control stays outside the renderer
leaf. The user manager owns/reaps the guardian even if the app dies. The transient unit remains runnable independently of
app scope teardown, has no PartOf/BindsTo dependency on the app, and kills all remaining service children on guardian
failure. The guardian opens verified cgroup.kill/events and
control handles, enables PR_SET_CHILD_SUBREAPER, and creates all job children itself. A small native launch shim enters
the leaf before exec and blocks at the ownership barrier; no Node, schema import or renderer runs in the shim.
The guardian writes cgroup.kill on lifetime-pipe EOF and waits for populated=0; a cgroup alone does not watch parent
lifetime ([cgroup.kill](https://docs.kernel.org/admin-guide/cgroup-v2.html#core-interface-files)).
The service stop/failure policy kills its entire remaining cgroup subtree if the guardian itself fails. The certified
unit uses KillMode=control-group, SendSIGKILL=yes, Restart=no and a stop timeout within R29's 2 s cleanup deadline;
guardian main-process exit must trigger that policy, even with live renderer descendants. Require
certified cgroup-v2 delegation, kill support, controller/resource access and user-service lifetime separation.
The supported trusted execution profile keeps renderer descendants inside that subtree and forbids explicit cgroup
migration or out-of-band launches; this is not a hostile-code containment claim. If any prerequisite is absent, fail
external_supervision_unavailable before code execution; no process-group fallback. This is the adopted Q3 answer.
Changing session/process group does not change cgroup membership. Trusted external JS with the person's privileges
is not an adversarial OS security sandbox; out-of-band service/WMI launches are outside the supported adapter profile.

On Windows the independent native guardian creates a non-breakaway Job Object with JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE.
It is the sole owner of the lifetime Job handle, never the renderer or app. It creates the direct child suspended
with PROC_THREAD_ATTRIBUTE_JOB_LIST so assignment happens at creation, then verifies membership and limits before
resume ([process creation attributes](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-updateprocthreadattribute)).
This removes a guardian-crash gap between uncontained creation and later assignment. Failure to create an
already-assigned suspended process refuses before renderer execution. Resume waits for the pre-exec handshake.
No BREAKAWAY_OK or SILENT_BREAKAWAY_OK. On app death, pipe EOF makes the guardian terminate/close the Job; if the guardian
crashes, kernel handle closure kills its contained processes. Job handles are non-inheritable; nested Job constraints
must certify successfully or execution refuses. Count active processes from Job queries, not completion messages alone
([Windows Job Objects](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects)). The app waits/reaps the
guardian on ordinary completion; after app death the guardian exits after cleanup and the kernel releases its process
object when the remaining handles close. Windows has no Unix zombie-reaping requirement.

The pre-exec protocol is the same on both OSes:

1. The app creates a bounded job identity and lifetime pipe. The app alone keeps its write end, marked CLOEXEC on Linux
   and non-inheritable on Windows. A mutex-protected spawn path prevents fork/handle-duplication races. Neither shim,
   guardian, renderer, sibling job nor descendants inherit any writer. Pass only the read end to the guardian through
   authenticated local IPC, and use explicit handle allowlists. Renderer stdout is not the lifetime channel.
2. The guardian establishes its independent lifetime, containment, limits and reaper, persists at most one recovery
   identity per live job and replies OWNED. The recovery record contains version, random nonce, service/cgroup identity
   or Windows Job identity, owner-instance identity and phase, at most 1 KiB per record, at most two live jobs and two cleanup records. After app EOF the guardian
   cleans all jobs and exits; it never waits for app restart. Use opened handles
   and verified ownership, not a recycled PID/path or process-name search, for recovery.
3. The guardian creates the blocked shim/suspended process inside containment and replies CONTAINED. The app sends
   COMMIT only while its grant, project generation and deadlines remain valid. The guardian checks lifetime EOF/cancel
   again before releasing exec/resume. Partial startup, lost ACK, pipe closure or timeout kills the contained process;
   the app never falls back to a direct spawn. All acknowledgements have a 5 s startup deadline.
4. Only then execute the exact program with argument arrays. The guardian reaps its direct children with waitpid and
   adopts/reaps orphan descendants on Linux until ECHILD, plus populated=0. The user manager reaps the guardian.
   Windows waits for the direct process handle to signal and for active Job count zero, then closes all handles.

R29 [P1/P2, terminal cleanup]. Success, nonzero exit, protocol error, invalid output, timeout, cancel, supersession,
revoke, project close/handover and app shutdown enter one cleanup path. Allow 500 ms graceful cancellation, then kill
the entire containment boundary. Abrupt parent death skips the grace period. A 2 s cleanup deadline applies from the
kill decision; permission revocation stops new execution immediately. Keep the independent owner alive until OS emptiness
and reaping are confirmed. No ready manifest, terminal success/failure/cancel notification, released reservation or
reusable execution slot before cleanup. Native SVG sessions finish/idle out through the same path.

If OS cleanup cannot complete by the deadline, report fixed supervisor_cleanup_failed, block new external work and
keep guardian/service recovery ownership active. Mark the gate failed, never call it a cleaned-up render. After parent
death there is no app response; the surviving guardian writes only a bounded fixed completion record after cleanup.
Delete the scoped recovery record after empty/reaped; crash recovery inspects only its owned records. Guardian RSS,
IPC and staging remain charged until retirement. The guardian is Kinewright-native memory, not exempt external RSS.

Guardian loss is itself a failed job and permanently disqualifies its output from ready publication, even if all PNGs
were received before the loss. The app's existing cleanup owner retains reservations, staging/source pins, input leases,
execution-slot ownership and a bounded recovery identity independently of the guardian's IPC. EOF/death is not a
cleanup acknowledgement. It observes Linux service/cgroup state through retained control identities, or retained
Windows process identities/handles without duplicating the lifetime Job handle. Only independent whole-tree emptiness
and required reaping allow resource retirement, exactly once. If that proof cannot be obtained after guardian loss,
stay cleanup-blocked with charges/pins held and new work refused; do not infer zero from a closed pipe or lost query.
The guardian's own process-memory charge retires on observed OS process exit, while remaining tree/staging/input
charges remain until their respective last owners retire. Recovery keeps the same accounting distinctions.

G7 uses the production launcher and guardian, not a mocked supervisor. A controlled native fixture forks a grandchild
and reports OS-verifiable PID/start identity plus cgroup/Job membership through a separate observer channel. Linux adds
setsid. Assert both processes existed and were contained before triggering each terminal cause. Force-kill the app,
leaving the independent guardian alive; observe it empty/reap the tree and exit without restarting the app. For startup
failure inject faults before OWNED, after containment and before/after COMMIT; pre-child failures assert zero execs,
post-child failures first assert a contained process existed. Also use a child that creates descendants before a bridge
READY handshake, then fail that handshake and assert they disappear. These two classes prevent a vacuous
"startup failed so there was no child" pass. Deliberately leak a lifetime writer and remove kill-on-close/cgroup.kill
in separate mutations; the forced-death gate must fail. Unavailable supervision refuses before any user-code marker.
Keep the prior success-with-lingering-descendant, failure, cancel, timeout and 100-cycle handle/thread witnesses.

G7-guardian-death is a separate case on both native OSes, with the app alive. Before injection, independently observe
the real guardian and fixture child/grandchild OS identities, their membership and a live held resource reservation.
Freeze the cooperative fixture's process creation so its reported descendant set is complete. Let it write staging
output, then force-kill the guardian itself, SIGKILL on Linux and TerminateProcess on Windows. Repeat after all expected
frame buffers arrive but before cleanup/publication acknowledgement. A renderer READY/exit record cannot authorize
publication after guardian death.

| OS | Independent fallback evidence after guardian death |
| --- | --- |
| Linux | Observe the systemd unit's main-process failure and stop policy, then cgroup.events populated=0 for the retained service/job subtree. Track each previously observed PID/start identity through exit and disappearance. The service manager reaps its guardian; orphan descendants must be reaped by the surviving OS reaper, PID 1 or the certified next subreaper. Assert no live or zombie fixture identities. The dead guardian cannot supply its own success record |
| Windows | Before injection record Job membership/count, non-breakaway/kill-on-close flags and the non-inherited sole lifetime handle. The observer retains only SYNCHRONIZE/query process handles for the complete fixture set, never a Job handle. After guardian termination, kernel last-handle closure must terminate every contained process; all retained process handles signal and no fixture identity remains active. Do not attempt a post-destruction Job query or treat its failure as active-count zero |

Barrier-pause the app's cleanup confirmation after the kill. Independently inspect its actual reservation/pin/slot
ledger, including native guardian/helper and external-tree reporting, and verify that still-owned resources remain
charged even after guardian IPC closes. Then release the barrier, establish OS emptiness/reaping and real resource
retirement, and check each reservation/pin/slot releases exactly once. The job remains failed and has no ready manifest,
success event or attachable final output. A delayed frame/cleanup ACK from the killed identity cannot change that.
In a separate observation-loss case withhold authoritative emptiness proof; cleanup-blocked retains charges and refuses
new work. Predecessor cached output stays readable and cannot be relabelled as this job's success.

Mutate systemd's whole-cgroup failure cleanup, remove Windows kill-on-close, or deliberately keep an extra lifetime Job
handle in separate runs; the corresponding observed-descendant death witness must fail. Separately mutate release on
guardian EOF and publication on buffered READY; the ledger/publication assertions must fail. Keep these mutations apart
from app-death and startup witnesses so guardian-failure coverage cannot pass via a different cleanup path. This is
required future evidence, not evidence collected during this documentation-only revision.

## 8. Registry and person workflow

R30 [P1/P2]. Propose six generated mutators: AddCodeClip, SetCodeSource, SetCodeParam, SetCodeParamCurve,
AddExternalClip, SetExternalSource. Reuse SetCodeParam for typed constant external props; refuse curves on External.
Three registry capabilities handle bounded asset import, supervised external describe/render/cancel actions, and bounded
code/render-state inspection. None is served directly. Permission grants have no capability and no Operation. Imports
write immutable assets; document changes still use exact prepared ops, revision checks, validation and one attributed
undo per commit. Operations carry source blob references and bounded descriptor/spec metadata, not SVG, Zod code or
inline frame data. AddCodeClip/SetCodeSource include the verified asset record needed to register it atomically in
`code_assets`; the External equivalents register their spec. There is no out-of-band document-table edit.

The required served quad is 7 tools / 5,660 serialized bytes / 3,510 schema bytes / 998 description bytes, with frozen
served inputs (AW0 §7:435-453). Capture and compare actual tool definitions byte-for-byte, not just those lengths.
Ledger registry growth and shared Operation `$defs` propagation; descriptions stay below 1,024 bytes. Serialize pin-site
landings after AW1, MO2, CC8 or AW3 work that touches the same registry (AW0 §12:704-709).

The person can add a generated SVG bundle, edit the same declared parameters, set numeric curves and undo. The inspector
shows missing/bad source or fonts, renderer status, stale output, byte usage and cancel. Enabling external code names
the project/runtime and the trust rule. No editor for SVG paths or TSX, runtime installer, template gallery or general
task dashboard is added.

## 9. Budgets and accounting

All new figures below are provisional under the adopted Q4 ruling. Certify fixtures and ceilings before acceptance. Keep separate Windows x64 MSVC and Linux x86_64
rows with release binary/compiler, CPU/GPU/backend, font hashes, fixture hash and exact dependency profile. Use the
stricter of these ceilings and any AW1/PF1 pins accepted before AW2. AW1's documented estimates are idle 400 MB, 1080p
proof 1 GB, proxy 120 MB, startup 3 seconds/proxy 1 second (`docs/AW1-HEADLESS-KINEWRIGHT.md:650-665`).

| Resource | Proposed ceiling and overflow policy |
| --- | --- |
| Source/description | SVG 1 MiB; 2,048 nodes; depth 32; 32,768 path segments; 32 declarations/bindings; 2,048 UTF-8 bytes per text value, 16 KiB total values; reject before parse/allocation growth |
| Curves/metadata | 1,024 keys per param, 8,192 per clip; 256 AW2 asset/spec records, 256 KiB combined serialized metadata per project; refuse growth, never prune keys to fit |
| Fonts | 8 MiB total loaded fonts per native session; reject missing coverage or admission above cap |
| Raster/output | Nonzero width/height, side at most 3,840, pixels at most 8,294,400; checked RGBA8 length at most 33,177,600 bytes; no scale-down disguised as full resolution |
| Native memory | Helper OS resource ceiling 512 MiB; desired working allocation at most 64 MiB + 24 bytes × pixels; parent conversion/cached allocations charged separately |
| Native time | Source preparation at most 1 s; frame watchdog 100 ms preview, 1 s proof/export, plus at most 2 s cleanup; no late frame accepted |
| Native live scrub | On each named release performance lane: 600 changing-param 1080p frames, cold cache p95 at most 8 ms raster/bind/parse/IPC; full preview p95 at most 16.67 ms, at least 60 displayed fps after warmup; include guardian/helper startup in a separate cold-session row; warm-cache p95 at most 2 ms for lookup/hand-off |
| Kinewright RSS | Headless post-open at most 400,000,000 bytes; one 1080p proof peak at most 1,000,000,000; proxy at most 120,000,000. Include native helper and guardian peaks conservatively; keep external child tree separate |
| Pixel cache | Preview Code and PNG working rasters enter PF1's shared K-1/K-2 224 MiB required-set accounting. Synchronous proof/export retains K-6 policy and separate live reporting, with no claim of I12. At most 8 MiB parsed template/tree/font-index cache, at most 32 entries, included in RSS |
| Disk ownership | 8 GiB per-project AW2 store including pins, scratch and in-flight reservations; at most 2 GiB one render. Per-frame PNG cap 40 MiB; at most 1,000 frames; manifest at most 256 KiB. Refuse admission or stop before the next capped write; referenced assets never evicted |
| External work | One active job globally, 16 queued jobs/256 KiB queue metadata, 64 retained job summaries; one SVG worker and one queued SVG request, newest preview wins |
| Immutable inputs | R24 source blobs, source-image bytes and staging copies reserve actual disk bytes within the existing 8 GiB project cap; charge overlapping copies separately. Person-owned runtime/dependency images remain outside the project, with their disk footprint reported. Native image-lease/manifest buffers count in Kinewright RSS; no unbounded full-tree buffer |
| External deadlines | Probe 5 s; describe 15 s; bundle 60 s; frame progress 30 s; total render 10 min; watchdog includes hung JS, not just delayRender's timeout |
| Pipes/status | 64 KiB ring per stdout/stderr, 1 MiB maximum framed bridge message, 4 KiB status response, 4 updates/s; discard extra logs while draining; cap errors as in Part 0 |

### 9.1 Accepted PF1 integration, not rev1 eviction accounting

The integration target is accepted PF1 §5 K-1/K-2/K-6. The inspected pf1/impl rules at
`docs/PF1-PLAYBACK-PERFORMANCE.md:154-217` take reservations before conversion, admit the entire required preview set
atomically, release at the last owner, and retain a distinct synchronous policy. The concrete seams are PF1
`crates/kinewright-media/src/render.rs:424-444,472-574` reader_demand/generated keys,
`preview.rs:122-190,1832-1860,1952-1982` Hold/Pinned and admission,
`sched.rs:1995-2078` Readers::admit, and `render.rs:937-955` generated cache inventory/drop.
Extend those owners to Code keys and External frame keys. Do not bolt another cache allowance onto a synchronous
renderer and call it scheduler integration. Inferred implementation shape: generalize the generated-key inventory to
include SVG content/params and PNG render-key/frame/color context, retaining identity-based accounting.

For preview, include every required Code raster and External stored frame in the distinct required set before doing
IO/conversion or starting a helper frame. Admission reserves the whole set under Sched. If it fits C but live+q does
not, apply PF1 draining/cancel/evict rules and wait only for live ownership; drop victims and release guards outside
Sched. If the set exceeds C or the accepted reader rule, take the existing counted K-3 path. Never partially reserve
one source at a time and deadlock with other retained frames. Each unique WorkingFrame allocation is charged once,
by shared_buffer_id, while cache, handoff, compositor pin or cancelled in-flight work retains it. Extend the existing
Hold/Pinned/drop-owner mechanism, not an eviction-time subtraction. Avoid early release at publication or cancellation.

Maintain a separately capped native scratch reservation, included in Kinewright RSS, before helper dispatch, compressed
PNG read/decode, IPC receive, unpremultiply and working conversion. Reserve measured/pessimistic concurrent peaks:
helper pixels, one bounded parent RGBA staging allocation, working f16 pixels, and at most one 1 MiB IPC chunk. Shrink
only when the actual old allocation retires. Final WorkingFrame bytes transfer into the admitted K-1 reservation
without a gap or second charge; scratch allocations keep their own charge while overlapping. IPC duplication charges
both actual allocations. Helper and guardian heaps count as native RSS; Node/browser heaps stay separately reported.
Separate scratch accounting does not exempt a retained working frame from K-1.

Use PF1's existing GpuLedger/TexturePool, submit_tracked, frame_poll and retirement owners
(`crates/kinewright-media/src/compositor.rs:190-217,819-868,2407`; `display.rs:517`). Charge textures, upload staging,
readback and in-flight/evicted resources until completion and final-owner release. Preserve weak CPU allocation
identity through residency, not a raw pointer that may be reused. No new AW2 completion service or second GPU ledger.
K-6 proof/export/thumbnails keep synchronous FrameRenderer policy and report live bytes separately; a simultaneous
proof and preview has both charges and contributes to the summed native RSS. The synchronous allowance is not silently
subject to preview I12 and is not subtracted from preview live.

G9-identity retains exact old Code and External WorkingFrame identities, handoff clones, export pins and their GPU
resources while clearing/evicting caches, cancelling an obsolete job, and publishing a different frame. Pause GPU
completion at a production submission boundary, admit new work under a small controlled cap, and independently sum
live allocations/reservations by identity. Assert live<=C for the scheduler path, all-or-nothing required-set admission,
no double-charge for an Arc clone, retained old bytes still charged, and separate native scratch and K-6 columns.
First drop the cache references: retained identities/bytes remain. Then release CPU owners: GPU in-flight charges remain.
Only after observed GPU completion and real retirement may those GPU charges disappear. Release final CPU/export pins
and assert each charge returns exactly once and admission wakes. Repeat cancellation/device-loss/pre-submit-error
paths without treating a poll error as completion. Mutations subtracting on eviction, dropping a cancellation charge,
partially admitting a set or releasing GPU bytes at submit must each fail this witness. Aggregate RSS or cache-entry
counts alone cannot satisfy it.

Measure Kinewright parent, guardian and native helper peaks separately and report their conservative sum; measure external
Node/browser/descendant RSS and time too, without a pass/fail RSS ceiling, as AW0 §5 requires (333-338). Keep that child
cost visible. Do not claim host memory is bounded by Kinewright's own ceiling. Per-frame gates include preparation,
parsing, rasterization, conversions and IPC, with cold and warm lanes distinguished. CI correctness lanes have the
watchdogs; 60-fps acceptance needs controlled named machines on both OSes, never a shared-runner timing claim. Timing
waits for Riel's quiet window under `target/review/PITFALLS.md` §Timing.

## 10. Acceptance gates, runtime reds and regression pins

### 10.1 Evidence classes and baseline-compatible entry points

Every gate is unrun in this design. "Runtime red" means a successfully built, successfully started test reaches a
named behavior assertion and fails for the intended missing/wrong behavior. Missing devices/runtimes, unknown OS
prerequisites, compilation errors, broken fixture setup and skipped/ignored tests are partial, never runtime red or green.
The binding witness discipline is `target/review/PITFALLS.md` §Tests and evidence.

Use today's public entry points: authenticated HTTP POST and AW1 stdio once its accepted predecessor exists;
`tools/call` for `get_capability`, `invoke_capability`, `prepare_edit_plan` and `commit_edit_plan`
(`crates/kinewright-agent/src/server.rs:1206-1261`), project `load_document`
(`crates/kinewright-project/src/project.rs:562`), existing project IO/journal recovery and analysis
`monitor_proof_for_document` / `preview_solo` (`server.rs:1581-1593,3986-4002`).
A test may carry future content/operations as literal JSON fixture bytes. It must not name a future Rust variant in the
baseline test source. The baseline must build. A small test adapter can report exact runtime outcomes at these same
entry points, but cannot synthesize pixels, change model admission or substitute a fake supervisor.

Name expected assertion failures in advance. The capability-open reds below fail on missing support; they do not prove
individual new invariants are wrong on the old model. After capability support exists, collect distinct targeted
mutation reds for those invariants. Each negative fixture first runs the corresponding valid Code/External or transport
control to its successful render/commit; rejection of every new clip can never turn the gate green. Record actual raw
baseline errors, named failed assertions and mutation failures alongside OS/source/fixture hashes.

| Runtime-red gate | Existing entry and valid control | Exact expected failing behavior assertion on the predecessor | Targeted post-implementation mutation |
| --- | --- | --- | --- |
| G0 bounded transport | Raw POST and real stdio, ordinary valid request plus G0d consent/result/error controls | `G0_INVALID_ID_IS_MINUS_32600`: invalid-id path silently accepts or uses another code; `G0_NO_CANARY_BEFORE_LOG`: mismatch canary reaches output/log. Confirm each on the predecessor; carried defects alone are not reproduced evidence | Remove id guard, correlation, per-request version selection, pre-log sanitization, reader/depth/deadline or egress bound independently |
| G1 native SVG | Load literal v3 Code project through load_document, then existing monitor proof with requested raster; Solid/Title project proves working renderer control | `G1_SVG_RAW_FRAME_PRESENT`: valid Code fixture cannot load/render on the predecessor, so required raw frame is absent. Then expected equality is a separate assertion once support exists | Perturb one binding, font hash, renderer profile or unpremultiply byte; require raw byte comparison to fail, not just output length |
| G2 Code owners | Literal JSON operations through prepare_edit_plan/commit_edit_plan, then ordinary trim/split and proof. Same operation path with a Solid control succeeds | `G2_CODE_PLAN_COMMITS`: predecessor cannot admit AddCodeClip. After support, `G2_TRIM_CYCLE_SAME_PARAM_PIXELS` must compare restored evaluated values and pixels | Omit the Code-param owner from head-trim rebase, split-right shift or copy/undo traversal. Distinct mutation red for each changed owner |
| G2 External finite mapping | Literal v3 numbered-frame project through load_document and normal edit operations; same eight PNG truths verified directly plus Solid edit control | `G2_EXTERNAL_NUMBERED_2_TO_6`: predecessor lacks External admission; after support the finite equation and each R20 edit have exact tuple/pixel assertions | Remove finite bound at commit/load/replay separately; reset right origin on split, allow shorter replace, or make exclusive N visible. Valid base must still play first |
| G3 fail closed | Existing asset import/invoke + prepare/commit/load/replay; legal bundle and props commit/render first | `G3_VALID_GENERATED_CONTROL_COMMITS`: baseline lacks the positive capability; refusal subcases are not called red until that succeeds | Remove unbound-id, property type/bound, metadata-isolation, UTF-16 length, disk-reservation or watchdog check. Invalid edit must preserve revision/undo |
| G5 Remotion integration | Existing discovery/invoke wire path obtains proposed registry describe/render action; current get_timeline_state control succeeds | `G5_EXTERNAL_DESCRIBE_READY`: predecessor lacks that registry action. Once present, require schema describe, a typed prop revision, full render, ready manifest and cached proof all succeed, then reject bad props | Omit output verification, real-Zod recheck, source seal/namespace pin, Node/browser/backing lease, runtime tuple check or cleanup-before-publication independently; use R24's A/B replacement and change-then-restore controls |
| G6 renderer-free use | Copy a completed fixture's project/assets through shared project IO, then load, seek, proof and export. A copied media/Solid control succeeds under an exec-spawn tripwire | `G6_CACHED_EXTERNAL_EXPORT_COMPLETES`: predecessor cannot load/render the ready External project; once supported, no-runtime/spawn-tripwire export must complete | Add a probe/schema import during open/proof, evict referenced frames, select stale key or use old project absolute asset root. Missing-runtime negative alone never passes |
| G7 real process lifetime | Invoke the actual production launch/supervise API through a wire job, with the native descendant fixture replacing only renderer executable. Require observed descendants first; app death and guardian death are separate injections | Initial implementation's forced-parent-death baseline is measured before the cleanup change. If no pre-existing supervisor exists, mark capability red separately; only guardian mutations establish cleanup reds | Leak lifetime writer; bypass pre-exec containment; skip cgroup.kill/systemd failure cleanup or kill-on-close; retain an extra Job handle; release charges on guardian EOF; publish buffered READY after guardian death. Use real OS processes and actual ledgers |
| G8 enablement | Existing wire/GUI/headless person path, grantless exec marker and later person-granted successful render | Predecessor lacks executing action; do not count that refusal as permission enforcement. After support assert `G8_DENIED_EXEC_COUNT_ZERO` and granted positive control | Bypass grant on version probe/schema import, accept stored grant or skip revoke cancellation |
| G9 ownership/budgets | Real PF1 preview plus synchronous proof/export, R20/G1 pixels and retained identities from §9.1 | Use PF1 accepted byte reporting control. New Code/PNG workload gets a feature-open red until supported; then targeted identity mutations must produce `G9_RETAINED_IDENTITY_STILL_CHARGED` | Eviction-time uncharge, release on cancel/publication, partial admission, early GPU retire; all independent from aggregate RSS/timing measurements |

### 10.2 Full exit matrix and lanes

The following are acceptance/regression pins, not claims that all tests fail on today's runtime. Preserve existing pins
where the feature already works. G10 is explicitly a regression pin and can be green before AW2.

| Gate | Required witness and comparison |
| --- | --- |
| G0 transport | All §0.4 rows G0a-G0e, both CI OSes, literal raw wire/log checks, malformed body version plus missing-header precedence, specialized-schema deviation/client rejection, controlled readers, successful elicitation and valid client error; no handler-only substitute |
| G1 SVG bytes | `aw2-svg-lower-third-inter-v1`: repeated cold/warm runs on Windows x64 MSVC and Linux x86_64 CI, all 600 sampled evaluated-param frames. Compare exact width/height, alpha contract and raw RGBA bytes/SHA-256 before GPU/color conversion, not PNG container size. Use nonopaque backgrounds and text to expose errors |
| G2 edit survival | Code trim-in/trim-out, split, roll/slide/move, copy attributes, undo/redo and replay preserve parked values, keep-outside numeric curves and visible proof pixels. External's independent eight-numbered-frame fixture exercises every R20 mapping and boundary through commit/load/replay |
| G3 refusal | Over-budget SVG/source/helper time/memory/output, unsupported SVG, unbound/duplicate ids, ill-typed/unknown/out-of-bounds props and inconsistent metadata refuse after a valid positive. No partial publication, revision or undo change. Include malformed PNGs and resource-ledger early returns |
| G4 composition | Native raster and stored External pixels enter the managed pipeline. Transparent edges over black and a distinct saturated background, non-Normal blend, adjustment, effect/enable curves and transition have reference pixel comparisons under accepted MO2/CC8 color contracts. No opaque foreground hides alpha errors |
| G5 real external render | Both certified Windows/Linux source/runtime image profiles: real Remotion schema describe, prop commit, numbered/alpha composition render, ready cache, repeated key hit with zero child starts, ordinary cached playback. R24 A/B source/input/package/Node/browser replacement and change-then-restore witnesses, including pre-existing writers and execution identity checks. Unsupported stability and metadata drift refuse. Source/prop changes create a new key; valid old key remains readable |
| G6 no renderer required | Copy project plus owned assets to fresh root, revoke permission, remove Node/Remotion/browser and original source; enable exec-spawn tripwire. Open, seek first/middle/last, proof and export succeed using stored truths. Compare decoded export/proof pixels and duration with named delivery tolerances, not encoder-container bytes. A source/prop edit reports pending or missing-runtime, never old final pixels |
| G7 containment/reaping | R28-R29 fixtures on both native CI OSes: actual descendants, setsid on Linux, success with lingering child, failure, malformed output, hung probe, cancel, timeout, revoke, supersession, project handover/close/shutdown, forced-app-death, forced-guardian-death and startup faults at every handshake boundary. Guardian death requires independent systemd/cgroup or last-Job-handle cleanup evidence, retained accounting before verified retirement, zero live/zombie fixture identities and publication refusal even after all buffers arrive. Observation loss stays cleanup-blocked. Ports/pipes close; 100 cycles preserve thread/handle bounds. Real Remotion repeats cleanup on integration lanes |
| G8 person grant | Open/copied project defaults disabled; MCP/investigator/plan replay/document fields/proxy cannot grant. No describe/version/import JS until grant. Grant permits job; revoke kills it and disables later jobs. Cached G6 needs no grant |
| G9 memory/time/store | §9.1 identity witness plus release RSS/frame/cache rows for each named machine. Cold start, idle, native proof, 600 changing-param frames, External streaming ingest/cached play, 10,000-key churn, simultaneous proof/preview and full pinned disk. Count parent/helper/guardian, transient scratch, K-1, K-6, GPU and external tree separately. Full pinned store refuses new admission while G6 still passes |
| G10 served quad | Byte-identical full list_tools definitions and inputs on HTTP and stdio, at 7 / 5,660 / 3,510 / 998. New registry capabilities reachable only through discovery/invoke; record every addition and amplified Operation $defs byte |
| G11 persistence/races | Unchanged v1/v2 save bytes; v3 exact Code/External round-trip; actual previous reader behavior and overwrite refusal; unknown versions rejected. Journal begins v1-compatible and appends a v3 op with writer max 3. Fault each Save As copy/publish boundary; source/destination stay intact. Retained undo/export pins survive clear/full-store refusal/interrupted copies. Late render after undo/project switch cannot relabel output |

Windows and Linux CI establish correctness, bytes, supervision and watchdogs. Performance acceptance uses the provisional
Linux desktop lanes LL=lavapipe and LH=RTX 3090, and the local Windows 11 VM reference with its recorded backend,
provisionally WARP. Do not substitute hosted CI timing. Record physical CPU/RAM, VM vCPU/RAM/host, OS/kernel, compiler,
binary hash, driver/backend and fixture identity before any accepted ceiling. Preserve the stricter accepted PF1/AW1
limits and AW0 A1's SVG live-scrub 60-fps requirement. A provisional target missed on a specified lane needs an explicit
ruling, not a silent lower-resolution substitution.

The proposed fixture set is `aw2-svg-lower-third-inter-v1` at 1920x1080/60 fps/600 frames with changing position,
opacity and font size, explicit Inter text and alpha edges; `aw2-svg-limits-v1` for maximum source/nodes/paths/fonts and
4K scratch; `aw2-external-numbered8-v1` for R20's eight distinguishable frames; `aw2-remotion-alpha-props-v1` at
1920x1080/30 fps/120 frames, fixed props and alpha; `aw2-owned-identities-v1` for overlapping Code/PNG/proof/GPU pins;
and `aw2-store-churn-v1` for 10,000 distinct keys and pinned-store exhaustion. These are named design fixtures, not
files claimed to exist. Final source/project/input hashes and the Windows/Linux dependency image tuples remain release
inputs to approve before performance ceilings become acceptance numbers. No measurement is claimed here.

Raw logs, literal fixture JSON, native raster bytes, manifests, OS process identities, baseline/mutation summaries and
count ledgers belong under `target/review/aw/aw2/` during implementation. Follow the existing CI slow-test manifest
policy. A gate summary must distinguish passed, failed and partial per OS/backend; a missing native lane is partial.

## 11. Ordered stages

Line counts are not a budget, cap, forecast or stop rule (Riel, 2026-10-20). The 6,400-, 9,000- and 10,600-line figures in
earlier revisions and reviews are history only. Over-engineering is guarded by scope and review instead: every
production mechanism traces to a rule (R1-R31) or a gate; reviewers ask whether a simpler design holds the same gates;
each shared mechanism has one owner. Performance and memory are guarded by measurement: §9's ceilings, the accepted
AW1/PF1 limits, Kinewright RSS and per-frame cost. Stop and seek a lead ruling before a stage adds a mechanism,
dependency, process, persisted field or person setup step this design does not name, or when a measured ceiling fails.
Never remove refusal/guardian/accounting gates to make a stage smaller.

| Stage, in order | Production work | Exit / dependency |
| --- | --- | --- |
| P0 | Envelope/version/correlation/refusal guard; rmcp and HTTP-parser/logging patches; connection/header/body admission; shared bounded stdio seam | G0a-G0e and G10, including ordered multi-fault admission and documented client compatibility limitation; independent landing before new kinds |
| P1a | Core descriptors/params/curve owners; immutable code/font store, durable pins and copy-first Save As; v3/load/replay validation | G2/G3/G11 model/store controls; no exposed unrenderable kind |
| P1b-s | Independent Linux service/cgroup guardian; Windows Job guardian; lifetime-pipe handshake/reaping/recovery/accounting retention; native resource policy | Real supervisor G7 native fixtures, separate forced-app/guardian-death, startup failure, ledger retention and unavailable containment; precedes helper execution |
| P1b-r | SVG profile/bindings; raster helper and pixel conversion; PF1 ownership/admission/GPU integration; native IPC/launch integration | P1b-s accepted first; G1/G3/G4/G9 native, retained-identity controls |
| P1c | Registry mutations/inspectors; person parameter/curve workflow | Part 1 release: G0-G4/G9-G11 relevant halves and independent review |
| P2a, non-executing | External model/constant props; finite edit mapping/validation; offline PNG manifests/store/pins | Data-only G2 tuple/G3/G11 controls with prebuilt frame manifests. Numbered pixel G2 and renderer-free G6 wait for P2c's reader. No bridge import, version probe, Node, Zod evaluation or real-render G5 exit |
| P2b | Person grant/revoke and roots/identity checks; dedupe/queue/state/cleanup integration | P1b-s accepted; G8 positive/negative controls through real supervisor with native executable marker. Still no executing schema/Remotion bridge gate |
| P2c, executing integration | Bridge/describe/render protocol; definition-derived Zod and exact tuple certification; bounded source-manifest/image handoff; Linux source/runtime namespace and executable leases; Windows source/runtime/backing/executable leases; PNG reader/conversion/PF1 integration; ready/stale publication | P2a and P2b complete. Certify source and runtime images, executable identities and grant checks before import/probe. R24 replacement/change-then-restore G5 controls, then G6/G7/G8/G9; all execution supervised |
| P2d | GUI status/cancel/permission presentation; packaging/manual source-image/runtime setup/help; production ledger/docs/integration instrumentation | All gates on both OSes, person workflow checklist including image refresh, independent review |

Build shared production mechanisms once. P1a owns the generic immutable blob store, durable pins and transactional
Save As; P2a adds External frame-manifest/index/pin integration rather than another store. P1b-s owns both OS
guardians, lifetime handshakes, death fallback and cleanup/accounting recovery for SVG and External, including probes.
P2b reuses that owner. P2c adds one source-image handoff and two OS lease backends; each backend covers source,
dependencies, Node and browser across probe/describe/bundle/render. It does not build four independent snapshot systems
or a second guardian. Reuse accepted PF1 ledgers/retirement and AW1 auth/consent/IO seams. If image provisioning needs
an in-app builder/installer, it exceeds this design and needs a new design ruling. This section does not approve
purchases, person setup or runtime acceptance ceilings.

Accepted PF1 is an explicit implementation predecessor, not merely a branch to reread. Require the lead's PF1 acceptance
and merge, then reconcile K-1/K-2/K-6 and CPU/GPU ownership against its actual merged hash. The inspected pf1/impl hash
is a design target only. Require accepted AW1 shared IO/handler/stdio/auth/consent prerequisites and the landed MO2/CC8
format/color contract at each affected seam. Rebase budgets/citations if those predecessors change; do not build a rival
format predicate, ledger or completion owner. The roadmap puts PF1 ahead of AW2 and records the production sequence
(`docs/ROADMAP-AND-WORKFLOWS.md:857-878,886-895`). The lead's current order is binding:
AW2, then MO6 including Bezier, then MO3, then MO5. AW3 is non-blocking.

P0 stands alone; P1a/P1b-s/P1b-r/P1c together form usable Part 1; P2a-P2d together form optional Part 2. Model-first
commits remain on the work branch until a working renderer exists, so main never exposes blank/unsupported kinds.
Permission and independent supervision precede every executing bridge/version/schema gate. Dependency stability is
established before import, not certified after executing a mutable project. No stage runs or edits are authorized by
this design task. Implementation workers later use the repository workspace/CI checks, stage behavior witnesses and
independent review under the lead's workflow. This design worker starts no reviewers or runtime work.

## 12. Deferrals, rejected alternatives and risks

R31 [specified, deferred]. Reserve a language-neutral renderer request with language/API, immutable source/assets,
declared typed params, rational frame context, dimensions, colour/alpha contract and budget. The result is complete
pixels plus provenance or a typed refusal. API dispatch rejects unsupported tags; reserving names does not make them
loadable/renderable in AW2.

| Deferred item | Specified boundary and owner |
| --- | --- |
| Typst | Future AW code-depth slice: source plus hashed fonts/import modules, typed dictionary bindings rather than string substitution, no filesystem resolver, bounded single-page vector-to-raster result. Compile/layout cache keys include compiler/assets. No compiler dependency or shell invocation in AW2 |
| WGSL | Future AW code-depth slice: validated shader module, explicit entry point, declared uniform layout/typed bindings, bounded output. Adapter/device identity, GPU memory and execution watchdog require new gates; GPU bytes cannot inherit SVG's cross-OS promise. No arbitrary shader accepted in AW2 |
| Curves driving external props | Later AW slice: versioned frame-indexed sampled-prop input and new cache identity/time-mapping gates; v1 props stay constant |
| More adapters | Later AW slices: same describe/validate/render/provenance/output/supervisor contract for Manim, Motion Canvas, Blender |
| Lottie/general SVG import | MO6 or later AE slice; designer asset import, artboard/pen/path editing and SVG profile expansion are separate scope |
| Draw-on strokes, springs, templates | MO6/later motion-depth work. Learn from study synthesis §AW/MO6; do not port another SVG parser or JS expression engine (`target/review/refs/synthesis.md:29-40,50-56`) |
| Raster models/audio generation | Deferred generative-assets programme; AW2 neither installs models nor adds hosted generation |
| External audio, retiming, HDR source output | Later media/AW depth: v1 External is silent, fixed-rate and display-sRGB; managed composition still applies ordinary project effects |

Main risks are template reparse cost versus 60 fps, font/shaping differences, allocation peaks outside caches,
process-boundary availability, dependency identity drift, disk pressure with durable pins and platform version churn.
Each has a refusal and a gate above; none is resolved by this design alone. Avoid a native scripting runtime, silent SVG
feature dropping, mutable-path cache keys, detached timed-out workers, process-name killing, or an LRU that deletes a
project's only external output.

Lessons used, not competitor capability claims: retain typed curves instead of JS expressions and verify transparent
graphics over two different backgrounds (`target/review/tesseract/study.md` §6 L4/L8); generated overlays complement an
edit rather than replace the editor (`target/review/video-use/study.md`, lesson 6); keep progressive discovery and
refuse silent test skips (`target/review/refs/synthesis.md:29-32,60-66`). No competitor installs or code were used here.

## 13. Rulings adopted and remaining questions

The lead adopted the critic's recommended answers 1-6. They are decisions in this revision, not questions to repeat.

| Rev1 question | Adopted answer and resulting contract |
| --- | --- |
| Q1 | Approve bounded ids and the correlation-only echo exception. R1-R5 now specify all four message shapes, request-scoped versions, refusals and consent/client-error controls |
| Q2 | Approve the native Rust helper exception. Its guardian/helper/IPC/conversion memory counts as Kinewright; real kill/reap witnesses apply |
| Q3 | Approve Linux fail-closed cgroup-v2 availability with independent guardian and certified provisioning. No process-group substitute |
| Q4 | Keep resource/timing figures provisional, use the Linux desktop LL/lavapipe and LH/RTX 3090 plus the local Windows 11 VM reference, name representative fixtures and preserve stricter accepted AW1/PF1 caps and 60-fps SVG |
| Q5 | Approve the finite Zod 4 subset and constant props, subject to exact tuple certification and faithful definition-derived constraints; metadata cannot override them |
| Q6 | Approve durable pins, full-store refusal and copy-before-publish Save As, with retained undo/export and interrupted-copy fault tests |

### Open for Riel

The verifier retained these three questions. They are open inputs, not requests to repeat adopted decisions.

1. Withdrawn (Riel, 2026-10-20): line-count forecasts and caps no longer apply; §11 states the scope and
   measurement rules that replace them.
2. Approve final fixture bytes/hashes, §9's provisional ceilings and the Windows 11 VM configuration/resources/backend.
   The machine classes are already chosen; measurements and final artifacts do not yet exist. Preserve stronger
   accepted AW1/PF1 ceilings.
3. Accept Part 2's person-provisioned immutable-image setup burden, including installed runtimes/OS utilities,
   read-only source/runtime images and a newly sealed source image after each source/input refresh.

Engineering owners select exact Node/Remotion/Zod/browser patches, image hashes and supported OS profile details, then
certify R24's leases/namespaces, faithful schema traversal and UTF-16 semantics and publish setup guidance. Those are
engineering inputs, not extra questions for Riel. Unsupported tuples/profile availability refuse. Chosen machines,
fail-closed containment, production order and the prohibition on bundled Node/Python need no renewed decision.

These are explicit promotion/implementation inputs. None authorizes a code change in this design-only round.

## 14. Source and verification notes

Rev2's source-inspection claims are preserved against its recorded baselines. Rev3 read the design, verifier and
PITFALLS, checked checkout metadata and read official upstream documentation for the specified edits. It did not reproduce a
transport bug, render a fixture, measure memory, launch a child/supervisor, run Cargo, edit tracked files, commit, push
or orchestrate another agent. The sole written artifact is this document in the ignored review directory. Citations
into the main checkout and PF1 refer to the baselines at the top; rebind them to accepted predecessors before coding.

Primary external references, read 2026-10-15:

- [UnsupportedProtocolVersionError](https://modelcontextprotocol.io/specification/2026-07-28/schema#unsupportedprotocolversionerror),
  whose required data.requested conflicts with AW2's stronger echo-free obligation. R4 names the documented omission
  and G0e tests client handling; specialized-schema conformance is expressly not claimed.
- [SquashFS](https://www.kernel.org/doc/html/latest/filesystems/squashfs.html),
  [fs-verity](https://docs.kernel.org/filesystems/fsverity.html),
  [Windows read-only virtual-disk flags](https://learn.microsoft.com/en-us/windows/win32/api/virtdisk/ne-virtdisk-attach_virtual_disk_flag)
  and [CreateFile sharing](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew).
  R24's complete image/namespace/lease setup is a proposed certified profile; these APIs alone do not demonstrate it.

- [rmcp 3.1.2 envelope model](https://docs.rs/crate/rmcp/3.1.2/source/src/model.rs), local registry model.rs §§JsonRpcMessage,
  JsonRpcResponse and JsonRpcError, especially lines 282-305, 491-510 and 668-685.
- [rmcp 3.1.2 HTTP implementation](https://docs.rs/crate/rmcp/3.1.2/source/src/transport/streamable_http_server/tower.rs)
  and [shared HTTP constructors](https://docs.rs/crate/rmcp/3.1.2/source/src/transport/common/server_side_http.rs).
  P0's inventory records the inspected constructors, including initialize logging before response construction.
- [2025-11-25 transport](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports),
  [2026-07-28 base messages/statelessness](https://modelcontextprotocol.io/specification/2026-07-28/basic),
  [versioning](https://modelcontextprotocol.io/specification/2026-07-28/basic/versioning), and
  [HTTP header validation](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/streamable-http#server-validation).
- [Linux cgroup-v2 kill operation](https://docs.kernel.org/admin-guide/cgroup-v2.html#core-interface-files) and
  [Windows Job Objects](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects).
  The guardian handshake, service lifetime, refusal and deadlines are AW2 design choices, not existing Kinewright behavior.
- [Zod Core](https://zod.dev/packages/core) and [JSON Schema metadata](https://zod.dev/json-schema#metadata).
  The finite supported checks and native constraints are an AW2 subset, not a promise to convert arbitrary Zod schemas.
- The docs.rs resvg-render/Pixmap links carried from rev1 were unavailable to the web reader. Read-only registry
  inspection confirmed `resvg-0.45.1/src/lib.rs:28-40` takes tree/transform/pixmap without a cancellation argument and
  documents sRGB; `tiny-skia-0.11.4/src/pixmap.rs:26,226-234` documents premultiplied RGBA. Those source contracts do not
  establish measured determinism or budget acceptance.
- [resvg 0.45.1 README](https://github.com/linebender/resvg/blob/v0.45.1/README.md) and
  [manifest](https://github.com/linebender/resvg/blob/v0.45.1/crates/resvg/Cargo.toml).
- [Remotion renderFrames](https://www.remotion.dev/docs/renderer/render-frames). Exact tuple/API behavior is a certification
  gate, not inferred from a broad version range. Mutable upstream documentation must be rechecked at that stage.

## Changes from rev1

This is rev2's historical change record. Its budget numbers describe rev2; §11 and Changes from rev2 give the current
forecast and contracts.

| Review item | Revision 2 change | Contract / witness |
| --- | --- | --- |
| Blocker 1, message admission | Disjoint request/notification/result/error shapes; outstanding-response correlation; mixed-shape/duplicate-key rejection; modern request scope and precedence | R1-R3; G0a/G0d, successful elicitation and valid client-error controls on both transports |
| Blocker 2, exact refusals | Literal constructor grammar, all codes/messages/statuses/dispositions and precedence; constructor/parser inventory; sanitize before initialize/Host/Origin/internal logging; raw wire plus controlled readers and exact boundaries | R4-R5; G0a-G0c, i64, 255/256/257-byte ids, depth 63/64/65, header/body caps, fifth reader and idle stdio |
| Major 3, parent death | Independent Linux cgroup guardian/user-service owner, fail-closed availability; Windows guardian and kill-on-close Job; pre-exec handshake, non-inherited writer, OS emptiness and reaping | R28-R29; G7 forced-parent-death/startup failure with descendants observed first |
| Major 4, External mapping | Exact finite selection equation, half-open bounds and all edit/replacement behavior, validated at commit/load/replay | R20; independent numbered-frame G2 fixture, undo/replay and targeted finite-bound mutations |
| Major 5, Zod agreement | Native descriptors derive from inspected definitions/checks; conversion metadata is disabled and never authoritative; UTF-16 string-length agreement | R23a-R23b; metadata/type/bounds/strictness/Unicode G3 controls, unchanged revision/undo on invalid edits |
| Major 6, PF1 ownership | Accepted PF1 predecessor; K-1 preconversion reservation, K-2 whole-set admission, last-owner retirement, distinct K-6 policy; helper/IPC/conversion/GPU accounting | §9.1; G9 retained identity through eviction/cancel/publication and real GPU retirement |
| Major 7, ordering/budget | Shared guardian before native execution; P2a non-executing, permission before bridge integration; AW2 then MO6+Bezier then MO3 then MO5, AW3 non-blocking; costed provisional 9,000 lines | §11, including 1,500 transport and 1,400 shared guardian lines |
| Major 8, evidence classes | Baseline-buildable public/wire fixture entries, positive controls and named runtime-red assertions; separate feature-open, invariant-mutation and regression pins; G7 real supervisor only | §10; G10 explicitly a regression pin |
| Open questions 1-6 | Adopt all critic recommendations and lead's provisional performance machine choice; retain only budget, fixture/ceiling approval and exact external tuple/provisioning inputs | §13 |

## Changes from rev2

| Verifier edit | Revision 3 change | Contract / required witness |
| --- | --- | --- |
| Major, version precedence | Separate syntax selection from refusal-code order; required HTTP mirrors precede malformed-body metadata, while stdio skips mirrors | R3/R4, G0b-version-precedence: literal version:7 plus missing header gives 400/-32020; valid header or stdio reaches -32602; valid modern discovery control |
| Major, unsupported-version compatibility | Explicitly choose omission of data.requested for AW2-OBL-1; explain specialized-schema nonconformance and unsupported strict-client refusal path | R4/G0e: exact echo-free bytes, generic-client failure handling, expected strict-schema rejection, no retry/downgrade/consent and supported-version positive control |
| Major, changing source and executable identity | Person-built per-refresh source images and reusable runtime images; Linux fs-verity/SquashFS plus pinned private namespace; Windows read-only VHDX plus backing/executable/path leases | R24/G5-stability: A/B source/assets/packages/Node/browser replacement and change-then-restore, pre-existing writer refusal, actual process/output identities and lost-lease accounting |
| Major, provisioning ownership | Person installs runtimes/image tools and refreshes immutable source images; Kinewright verifies bounded captured blobs/images; never bundle Node/Python or auto-install | R22/R24 and §13 Open for Riel item 3; engineering owns exact tuples and certification |
| Moderate, guardian failure | Kill guardian separately with contained descendants observed first, including after final frame buffers; independent OS cleanup proof, accounting retention and permanent publication refusal | R28/R29 G7-guardian-death: systemd/cgroup and reaping or kernel Job last-close/process-exit evidence; no observer Job handle; observation-loss and separate cleanup/ledger/publication mutations |
| Budget and shared mechanisms | Forecast 9,000 to 10,600: P0 +200, Part 1 model/store +300, shared guardian +300, P2c +700, P2d +100; count store, guardian and source/runtime lease backends once | §11: P0 1,700; usable Part 1 adds 4,700 for cumulative 6,400; optional Part 2 adds 4,200 for cumulative 10,600; reforecast at each part boundary |
| Questions for Riel | Preserve the verifier's three questions: forecast/stage caps, final fixtures/ceilings/Win11 VM, immutable-image setup burden | §13 Open for Riel; exact tuples/mechanism certification assigned to engineering |

All added witnesses and costs are design requirements and forecasts. No cargo, code change, runtime probe, process kill,
image provisioning or acceptance measurement was performed for rev3.
