# AGENTS.md

Instrucciones para agentes (humanos o IA) que trabajen **en este repositorio** o
que lo usen para consolidar una biblioteca real. La guía operativa completa
está en [`docs/playbook.md`](docs/playbook.md); el skill canónico para OpenCode
está en [`.opencode/skills/bye-bye-cloud-photos/SKILL.md`](.opencode/skills/bye-bye-cloud-photos/SKILL.md).

## 1. Qué es este proyecto

`bbcp` (Bye Bye Cloud Photos) es un CLI en Rust que:

1. inventaría una o varias raíces (álbum + exports de Takeout + volcados
   antiguos + papeleras) en SQLite;
2. agrupa copias exactas por contenido (hashes perceptuales para imágenes,
   SHA-256 para vídeos);
3. decide qué copia conservar, qué importar, qué actualizar y qué mover a
   cuarentena;
4. verifica por píxeles/fotogramas antes de borrar;
5. ejecuta copias atómicas y deja journal reanudable;
6. prepara lotes de borrado en Google Fotos solo para ítems con copia local
   verificada.

El objetivo no es "borrar rápido", sino **poder demostrar que no se perdió
nada**.

## 2. Invariantes que no se negocian

Cualquier cambio en el código debe preservarlas:

1. **Nada se borra sin una copia verificada.** Un `delete`/`external_covered`
   solo existe si el grupo tiene un miembro del álbum que sobrevive al plan.
2. **Todo es reversible por defecto.** Los duplicados del álbum se mueven a
   cuarentena; nunca se hace `unlink` salvo petición explícita.
3. **Las escrituras son atómicas.** Temporal en el directorio destino,
   `fsync`, SHA-256 origen/destino, `rename`.
4. **Las acciones destructivas son opt-in.** `apply` es dry run sin
   `--execute`. `external_covered`/`external_dup` no están en `--only` por
   defecto.
5. **El álbum no es automáticamente la mejor copia.** El ranking se basa en
   píxeles, bitrate/tamaño, antigüedad y tamaño.
6. **Sin cierre transitivo.** Un grupo es la terna exacta de hashes; encadenar
   similitudes une ráfagas y fotos distintas.
7. **Ante la duda, conservar.** Si la verificación falla o el archivo no se
   puede decodificar, la acción se convierte en `keep_distinct`.
8. **La base de datos no vive en 9p/NFS/FUSE.** SQLite + latencia alta +
   locking poco fiable es una combinación peligrosa.

Si una tarea parece requerir romper un invariante, para y pregunta.

## 3. Comandos de desarrollo

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release
```

El CI ejecuta exactamente eso en Linux y macOS. No se aceptan warnings de
clippy.

Prueba manual mínima (no requiere biblioteca real):

```bash
cargo run -q -p bbcp-cli -- init --album /tmp/bbcp-demo/Album --force
cargo run -q -p bbcp-cli -- doctor
cargo run -q -p bbcp-cli -- scan
cargo run -q -p bbcp-cli -- fingerprint
cargo run -q -p bbcp-cli -- group --reset
cargo run -q -p bbcp-cli -- plan
```

Para generar medios de prueba: `ffmpeg -f lavfi -i testsrc=size=320x240 -t 2 out.mp4`
y cualquier JPEG del sistema.

## 4. Mapa del código

| Archivo | Responsabilidad |
|---|---|
| `crates/bbcp-core/src/model.rs` | `FileRow`, `Action`, `MediaKind`, extensión → tipo |
| `config.rs` | `bbcp.toml`, valores por defecto, validación, config de ejemplo |
| `store.rs` | Esquema SQLite, upserts, plan, grupos, sidecars |
| `walk.rs` | Recorrido de raíces, filtros de basura, solapamientos |
| `hash.rs` | SHA-256 en streaming |
| `imagehash.rs` | pHash/dHash/aHash, MAD, fotogramas de vídeo |
| `video.rs` | `ffprobe` (resolución con rotación, duración, bitrate) |
| `group.rs` | Grupos exactos de imagen y vídeo |
| `dates.rs` | Sidecars, puente entre copias, EXIF, nombre, carpeta |
| `quality.rs` | Ranking y "mejora material" |
| `plan.rs` | Construcción de `plan_actions` |
| `verify.rs` | MAD antes de borrar; `compare` |
| `apply.rs` | Copias atómicas, cuarentena, journal |
| `stage.rs` | Stage de únicos y actualización de la BD |
| `google.rs` | Candidatos y lotes de borrado |
| `report.rs` | Informe Markdown |
| `lib.rs` | Módulos y `doctor` |
| `crates/bbcp-cli/src/main.rs` | Subcomandos y salida por consola |

Reglas de estilo:

* Rust 2021, `anyhow::Result` en la librería, mensajes de error en español
  (el CLI los imprime tal cual).
* Nada de `unwrap()`/`expect()` fuera de tests y de invariantes demostrados
  (por ejemplo un grupo no vacío).
* Las rutas del usuario no se asumen: siempre `strip_prefix` con fallback.
* Los cambios de esquema van en `SCHEMA` con `CREATE TABLE IF NOT EXISTS`; si
  cambia una columna de una tabla existente, documenta la migración.

## 5. Flujo al modificar el motor

1. Lee `docs/decisions.md` para entender por qué el diseño es así.
2. Cambia el módulo y añade una prueba en `crates/bbcp-core/tests/core.rs`
   (sin ffmpeg) o una prueba manual documentada.
3. Ejecuta `cargo fmt`, `clippy -D warnings` y `cargo test`.
4. Prueba el pipeline completo en `/tmp` con imágenes sintéticas **antes** de
   tocar una biblioteca real.
5. Si el cambio afecta a seguridad (borrado, cuarentena, journal, referencias),
   actualiza `docs/safety.md` y `AGENTS.md`.

## 6. Flujo al operar sobre una biblioteca real

1. **Preflight**: `./scripts/preflight.sh`, `bbcp doctor`, ownCloud/Syncthing
   parados.
2. **Inventario**: `scan`, `sidecars`, `probe`, `fingerprint`, `hash`, `group`,
   `dates`.
3. **Plan**: `plan`; revisa conteos y bytes. Si algo no cuadra, corrige la
   configuración y repite `plan`.
4. **Verificación**: `verify`.
5. **Dry run**: `apply` y `apply --only ...` para cada subconjunto.
6. **Ejecución aditiva**: `apply --execute --only upgrade,import`.
7. **Cuarentena**: `apply --execute --only delete --album-remove quarantine`.
8. **Revisión del stage**: `stage`, revisar, `apply --execute --only import`.
9. **Externos**: archivar JSON, después `--only external_covered`,
   `--only external_dup` con `--external-remove quarantine` o `unlink`.
10. **Google**: `google`, revisar `google_delete_candidates.tsv`, borrar por
    lotes.
11. **Cierre**: `report`, rescan, comprobar totales y espacio.

Nunca ejecutes los pasos 6–9 sin mostrar antes al usuario el resumen del dry
run y obtener confirmación explícita.

## 7. Trampas conocidas

* **9p**: cada `stat`/`rename` es lento. Paraleliza y evita `fsync` por
  archivo (el journal sincroniza cada 100 líneas).
* **Hashes degenerados**: imágenes lisas producen hashes con pocos bits.
  `min_popcount = 12` las excluye.
* **Ráfagas**: dos frames pueden compartir la terna. La verificación MAD los
  separa; no la omitas.
* **Vídeos "approx"**: misma duración y resolución no implica mismo vídeo
  (en una biblioteca real ~85 % eran distintos). Verifica por fotograma.
* **Fechas centinela**: `1970-01-01` y `1980-01-01` no son fechas.
* **Carpetas de volcado**: `2026/2026-01-05` es la fecha de importación;
  configúrala en `import_dump_days`.
* **Colisiones en cuarentena**: dos raíces pueden tener la misma ruta relativa.
  Son errores, no algo que se deba pisar en silencio.

## 8. Documentación y skill

* `docs/playbook.md` — flujo operativo completo con cifras de referencia.
* `docs/safety.md` — modelo de seguridad y modos de fallo.
* `docs/decisions.md` — decisiones de diseño y alternativas descartadas.
* `docs/google-takeout.md` — sidecars, fechas y lotes de Google.
* `.opencode/skills/bye-bye-cloud-photos/SKILL.md` — skill canónico de
  OpenCode. Si cambias el flujo, actualízalo en el mismo commit.

## 9. Licencia

GPL-3.0-only. Al reutilizar código de otro proyecto GPL, conserva la
atribución en `THIRD_PARTY.md`.
