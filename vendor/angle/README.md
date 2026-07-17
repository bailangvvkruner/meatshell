# Bundled ANGLE runtime

MeatShell uses ANGLE's EGL/OpenGL ES implementation to render through D3D11 on
Windows x64. The binaries come from the signed NuGet package below:

- Package: `Robust.Natives.Angle` `0.2.1-chromium7440`
- URL: `https://api.nuget.org/v3-flatcontainer/robust.natives.angle/0.2.1-chromium7440/robust.natives.angle.0.2.1-chromium7440.nupkg`
- Package SHA-256: `146275D3F9B698665478BFFCCEB752BE927609465326A7A3867439204751565F`
- ANGLE commit: `020c8ea3571f7b2084ef9a57c9e41b80fcc3f630`
- Build source: `https://github.com/space-wizards/native-build`

Vendored Windows x64 files:

- `libEGL.dll`: `41D933629CC0DD90190EAAC5DDFC65B822974C809F8B3E79E5ACA64B836D3B92`
- `libGLESv2.dll`: `ABBA3DB61100CA558BEB1DE2903719A29CCFFB89E54DBCB413D94F7C8EAD9AF7`

Run `scripts/verify-angle.ps1` after replacing either binary. Update all hashes
in that script and this file only after testing screenshots, window resizing,
DPI changes, idle CPU, process memory, and GPU process memory.
