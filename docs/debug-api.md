# Debug API

meatshell can expose an authenticated HTTP API for local AI agents and debug
tools. It is disabled by default and always binds to `127.0.0.1:24817`.

Enable it from **Settings > Debug API**, then copy the Bearer token. Every
request requires:

```text
Authorization: Bearer <token>
```

## Endpoints

- `GET /v1/health`
- `GET /v1/screenshot?max_width=2048&max_height=2048`
- `GET /v1/terminals`
- `GET /v1/terminals/{id}/screen?max_lines=200`
- `POST /v1/terminals/{id}/input`
- `POST /v1/terminals/{id}/pointer`

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
