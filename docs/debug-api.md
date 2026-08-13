# Debug API

MeatShell can expose an authenticated HTTP API to local diagnostics and AI
tools. It is disabled by default and always binds to `127.0.0.1:24817`; there is
no setting that exposes it on a LAN address.

Enable it under **Settings > Debug API**, then copy the generated token. The
token can be regenerated from the same page and is encrypted in the local
configuration store. Every route, including the health check, requires:

```text
Authorization: Bearer <token>
```

Responses use `Cache-Control: no-store` and `X-Content-Type-Options: nosniff`.
JSON explicitly declares UTF-8 so CJK and terminal box-drawing characters are
decoded correctly by Windows PowerShell 5 as well as modern clients.

## Endpoints

<!-- markdownlint-disable MD013 -->

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/v1/health` | Build, renderer, ANGLE, process-memory, and idle-trim diagnostics |
| `GET` | `/v1/screenshot?max_width=2048&max_height=2048` | Current rendered window as PNG without focusing it |
| `GET` | `/v1/terminals` | Public metadata for open terminals |
| `GET` | `/v1/terminals/{id}/screen?max_lines=200` | Newest visible terminal text |
| `POST` | `/v1/terminals/{id}/input` | Send UTF-8 text and an optional terminal Enter |
| `POST` | `/v1/terminals/{id}/pointer` | Send an xterm mouse event when tracking is enabled |

<!-- markdownlint-enable MD013 -->

Terminal metadata contains only `id`, `title`, `host`, and connection `state`.
The API does not enumerate saved sessions and never returns passwords, private
keys, key passphrases, or WebDAV credentials.

The health response includes:

- package version, build profile, and executable size;
- selected renderer, whether it is considered hardware accelerated, whether
  the packaged ANGLE `libEGL.dll` is loaded, and the 33 ms terminal scheduling
  interval;
- current/peak working set, private working set, and private commit where the
  operating system provides them;
- idle-memory-trim availability, delay, count, last result, and D3D trim state.

`angle_runtime_loaded` confirms that the ANGLE DLL is loaded; a renderer name
alone is not proof that ANGLE rather than another OpenGL path is active.

## Input

The input body is JSON:

```json
{
  "text": "pwd",
  "submit": true
}
```

`submit: true` appends one terminal Enter (`CR`). The request completes only
after the target transport acknowledges the bytes. A disconnected terminal
returns a conflict instead of silently accepting input.

Pointer input uses zero-based terminal cell coordinates. `kind` is `click`,
`press`, `release`, or `motion`; `button` is `left`, `middle`, or `right`.
`ctrl` and `alt` are optional modifier flags. A click can set `clicks` to `2`:

```json
{
  "kind": "click",
  "button": "left",
  "col": 72,
  "row": 14,
  "clicks": 2
}
```

A double click is sent as two complete press/release batches separated by at
least 120 ms. Pointer injection is rejected while the terminal application has
not enabled xterm mouse tracking, so an API click cannot become shell text.

## Limits

- Request bodies: 64 KiB; terminal input including optional Enter: 16 KiB.
- Outstanding inputs: four per terminal; additional input receives `429`.
- Screen text: 200 lines by default, 1,000 lines maximum, and 128 KiB maximum.
  When capped, the newest complete UTF-8 content is retained.
- Pointer coordinates: `0..=4095`; a click count is one or two.
- Screenshots: 4,096 pixels per edge and 16 million pixels after scaling.
  Aspect ratio is retained. Only one capture/encode operation runs at a time;
  overlap receives `429 screenshot_busy`.
- Input acknowledgement timeout: 10 seconds; screenshot capture and encoding
  share an 8-second deadline.

## PowerShell Example

```powershell
$headers = @{ Authorization = "Bearer <token>" }
$base = "http://127.0.0.1:24817"
$terminals = Invoke-RestMethod "$base/v1/terminals" -Headers $headers
$terminalId = $terminals.terminals[0].id

Invoke-RestMethod "$base/v1/terminals/$terminalId/screen?max_lines=100" `
  -Headers $headers
Invoke-RestMethod "$base/v1/terminals/$terminalId/input" `
  -Method Post -Headers $headers -ContentType "application/json" `
  -Body '{"text":"pwd","submit":true}'
Invoke-RestMethod "$base/v1/terminals/$terminalId/pointer" `
  -Method Post -Headers $headers -ContentType "application/json" `
  -Body '{"kind":"click","button":"left","col":72,"row":14,"clicks":2}'
Invoke-WebRequest "$base/v1/screenshot?max_width=2048&max_height=2048" `
  -Headers $headers -OutFile meatshell-debug.png
```

Treat the token as a local secret. Loopback binding prevents direct remote
access, but software running as the same user can still call the service if it
obtains the token. Disable the API when it is not needed and regenerate the
token after sharing logs or settings screenshots that might contain it.
