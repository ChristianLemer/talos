# native/ — pty-ffi libs (version EXACTE 0.40.0)

`deno compile --self-extracting` extrait ces libs sur le disque au 1er run
(dlopen ne lit pas le FS virtuel du binaire). Une lib d'une AUTRE version →
`symbol not found: pty_read_bytes`. Source : releases de `sigmaSd/deno-pty-ffi` 0.40.0.

| Fichier | Cible | Statut |
|---|---|---|
| `libpty_arm64.dylib` | aarch64-apple-darwin | ✅ présent (du spike) |
| `pty.dll` | x86_64-pc-windows-msvc | ✅ présent (du spike) |
| `libpty_x86_64.dylib` | x86_64-apple-darwin | ❌ à récupérer |
| `libpty.so` | x86_64-unknown-linux-gnu | ❌ à récupérer |

Les deux manquants ne bloquent que `build:mac-x64` / `build:linux`. Le dev sur
Mac arm64 et `build:win` tournent déjà. À combler en Phase 1 (CI release).
