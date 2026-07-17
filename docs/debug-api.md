# Debug API

meatshell can expose an authenticated HTTP API for local AI agents and debug
tools. It is disabled by default and always binds to `127.0.0.1:24817`.

Enable it from **Settings > Debug API**, then copy the Bearer token. Every
request requires:

```text
Authorization: Bearer <token>
```

JSON responses explicitly declare UTF-8 so terminal box drawing and CJK text
decode correctly in Windows PowerShell 5 as well as modern HTTP clients.

## Endpoints

- `GET /v1/health`
- `GET /v1/screenshot?max_width=2048&max_height=2048`
- `GET /v1/terminals`
- `GET /v1/terminals/{id}/screen?max_lines=200`
- `POST /v1/terminals/{id}/input`
- `POST /v1/terminals/{id}/pointer`

The health response includes the build profile and executable size, current and
peak process memory, selected renderer, whether it is GPU-backed, whether the
bundled ANGLE EGL runtime is actually loaded, and the active terminal render
interval. These fields let automated checks catch an accidental debug binary,
track memory growth, and distinguish the D3D11 path from a software or
native-OpenGL fallback. `working_set_bytes` is total resident memory,
`private_working_set_bytes` is resident memory private to MeatShell, and
`private_commit_bytes` includes committed pages that are not currently
resident. The memory-trim object reports idle heap/working-set reclamation and
whether ANGLE's D3D11 device accepted `IDXGIDevice3::Trim`.

The input request body is JSON:

```json
{
  "text": "pwd",
  "submit": true
}
```

`submit: true` sends one terminal Enter (`CR`). Requests are size-limited,
screen responses are capped, and at most four inputs may wait for a terminal at
once (`429` when busy). The API never returns saved passwords, private keys, or
WebDAV credentials.

Pointer input uses zero-based terminal cell coordinates and is accepted only
while the terminal application has enabled xterm mouse tracking. This keeps a
debug click from becoming shell input. `kind` is `click`, `press`, `release`, or
`motion`; `button` is `left`, `middle`, or `right`. A click may set `clicks` to
`2` for a deterministic double click:

```json
{
  "kind": "click",
  "button": "left",
  "col": 72,
  "row": 14,
  "clicks": 2
}
```

A double click is dispatched as two complete press/release batches separated
by at least 120 ms. The SSH transport also paces consecutive presses while it
continues reading remote output. This prevents Linux TUIs such as btop from
reading both presses in one input poll and treating them as text selection.
Local, serial, and Telnet sessions keep their normal pointer pass-through; this
is not a Windows btop compatibility mode.

The screenshot endpoint captures the currently rendered MeatShell window
without focusing it and returns `image/png`. Dimensions are aspect-ratio
preserving and capped by both query limits and the server pixel limit. Only one
capture/encode operation may run at a time; an overlapping request receives
`429 screenshot_busy` instead of building an unbounded image queue.

## PowerShell Example

```powershell
$headers = @{ Authorization = "Bearer <token>" }
$terminals = Invoke-RestMethod http://127.0.0.1:24817/v1/terminals -Headers $headers
$id = $terminals.terminals[0].id

Invoke-RestMethod "http://127.0.0.1:24817/v1/terminals/$id/screen?max_lines=100" -Headers $headers
Invoke-RestMethod "http://127.0.0.1:24817/v1/terminals/$id/input" `
  -Method Post -Headers $headers -ContentType "application/json" `
  -Body '{"text":"pwd","submit":true}'
Invoke-RestMethod "http://127.0.0.1:24817/v1/terminals/$id/pointer" `
  -Method Post -Headers $headers -ContentType "application/json" `
  -Body '{"kind":"click","button":"left","col":72,"row":14,"clicks":2}'

# Capture the current rendered window without focusing it. The API returns PNG.
Invoke-WebRequest "http://127.0.0.1:24817/v1/screenshot?max_width=2048&max_height=2048" `
  -Headers $headers -OutFile meatshell-debug.png
```

Only an AI client with local tool or HTTP access can call this interface; a
standalone model cannot initiate a connection to the computer by itself.
