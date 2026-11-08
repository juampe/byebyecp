---
name: bye-bye-cloud-photos
description: Consolidar una biblioteca de fotos y vídeos en la nube (Google Takeout, volcados antiguos, álbum local) con `bbcp`: inventario SQLite, deduplicación por contenido, elección de la mejor copia, verificación por píxeles/fotogramas, cuarentena reversible, journal y lotes de borrado de Google Fotos. Úsalo cuando haya que liberar espacio o unificar bibliotecas sin perder archivos.
---

# Bye Bye Cloud Photos — skill operativo

Herramienta: `bbcp` (Rust). Repositorio: <https://github.com/juampe/byebyecp>.
Toda la configuración vive en `bbcp.toml`; todo el estado, en `.bbcp/`.

Este skill es autosuficiente. No requiere documentos previos ni scripts
externos.

## Cuándo usarlo

* Consolidar varios exports de Google Takeout en un único álbum local.
* Deduplicar una biblioteca conservando la mejor copia.
* Liberar espacio de forma reversible (cuarentena) antes de borrar en la nube.
* Preparar lotes de borrado en Google Fotos con prueba de copia local.
* Revisar por qué dos archivos se consideran iguales o distintos.

## Reglas de seguridad (leer antes de ejecutar)

1. **Nunca borrar sin copia verificada.** El plan solo propone borrados cuando
   otro miembro del grupo sobrevive; aun así, revisa el dry run.
2. **`apply` es dry run sin `--execute`.** Enseña el resumen al usuario y pide
   confirmación antes de ejecutar.
3. **Los borrados del álbum van a cuarentena** (`--album-remove quarantine`,
   por defecto). No uses `unlink` salvo que el usuario lo pida y se haya
   verificado el espacio/backup.
4. **Las fuentes externas no se purgan por defecto.** `external_covered` y
   `external_dup` exigen `--only` explícito.
5. **Antes de purgar un Takeout, archiva sus JSON** (metadatos y google_id).
6. **Para si la verificación falla.** `keep_distinct` significa "revisar a
   mano"; no lo fuerces.
7. **No toques documentos, ROMs ni archivos que no sean imagen/vídeo.**
8. **La BD debe estar en un sistema de archivos local.** Nunca en 9p/NFS/FUSE.
9. **Detén la sincronización** (ownCloud, Syncthing, Drive) mientras mueves
   archivos del álbum.

## 0. Preflight

```bash
bbcp doctor
df -h /ruta/Album          # espacio libre donde vive el álbum
df -h .                    # espacio libre donde vive .bbcp/
```

`doctor` comprueba `ffmpeg`, `ffprobe`, SQLite, permisos del álbum y existencia
de cada raíz. Si falta ffmpeg/ffprobe, instálalos antes de continuar.

Configuración mínima (`bbcp.toml`):

```toml
album = "/ruta/Album"
metadata_archive = ".bbcp/metadata-archive"

[paths]
db = ".bbcp/bbcp.db"
quarantine = ".bbcp/quarantine"
staging = ".bbcp/staging"
logs = ".bbcp/logs"
manifests = ".bbcp/manifests"

[[roots]]
label = "album"
path = "/ruta/Album"
kind = "album"

[[roots]]
label = "takeout-2026"
path = "/ruta/Takeout/Takeout/Google Fotos"
kind = "source"

[rules]
min_popcount = 12
aspect_tolerance = 1.05
mad_threshold = 1.5
material_upgrade_ratio = 1.25
frame_distance = 16
duration_tolerance = 0.5
ignore_dates = ["1970-01-01", "1980-01-01"]
import_dump_days = []   # p. ej. ["2026-01-05"] si el Takeout volcó todo ahí
```

Registrar raíces nuevas:

```bash
bbcp scan --root takeout-2026
```

`scan` sincroniza las raíces del TOML con la BD automáticamente.

## 1. Inventario y fingerprints

```bash
bbcp scan                 # todas las raíces; --prune borra filas desaparecidas
bbcp sidecars             # JSON de Takeout → tabla sidecars
bbcp probe --workers 16   # metadatos de vídeo (ffprobe)
bbcp fingerprint --workers 16
bbcp hash --workers 16    # SHA-256 solo de vídeos con tamaño repetido
bbcp group --reset
bbcp dates
```

Orden importante: `group` **antes** de `dates`. Así una copia sin metadatos
hereda la fecha de su gemela exacta (puente `sidecar-copy`).

Comprobaciones útiles:

```bash
bbcp status
sqlite3 .bbcp/bbcp.db "SELECT media, COUNT(*) FROM files GROUP BY media;"
```

## 2. Plan y verificación

```bash
bbcp plan
bbcp verify --workers 8
```

Acciones posibles:

| Acción | Significado |
|---|---|
| `keep` | Se queda donde está |
| `keep_distinct` | Parecía duplicado pero la verificación lo separó: revisar |
| `delete` | Duplicado del álbum → cuarentena |
| `upgrade` | Copia mejor en una fuente → sustituye la del álbum |
| `import` | Media única de una fuente → copiar al álbum |
| `external_covered` | Duplicado de una copia protegida del álbum → purgar fuente |
| `external_dup` | Duplicado dentro de una fuente → purgar tras importar la mejor |

Revisa el resumen (conteos y bytes). Si algo no cuadra, corrige la config y
repite `plan`; no hay nada escrito en disco todavía.

## 3. Ejecución

```bash
bbcp apply                                    # dry run
bbcp apply --execute --only upgrade,import    # aditivo y verificado
bbcp apply --execute --only delete --album-remove quarantine
```

Qué hace `apply`:

* copia a `.bbcp-tmp-*` en el directorio destino, `fsync`, compara SHA-256 y
  `rename` atómico;
* mueve el destino antiguo de un `upgrade` a cuarentena;
* mueve los duplicados del álbum a `.bbcp/quarantine/<fecha>/album/<ruta>`;
* escribe `.bbcp/logs/apply-<fecha>.tsv` (reanudable).

`--consume-sources` elimina la fuente tras una copia verificada. Úsalo solo si
el álbum está en otro disco y el usuario lo aprueba.

Si hay errores, el comando termina con código distinto de cero y deja el
journal con la causa. Corrige y vuelve a ejecutar: es idempotente.

## 4. Stage de revisión

```bash
bbcp stage --root takeout-2026           # dry run
bbcp stage --root takeout-2026 --execute
bbcp stage --list
```

Mueve los `import` a `.bbcp/staging/<fecha>/<raíz>/<ruta>` y actualiza la BD.
Después:

```bash
bbcp apply --execute --only import
```

Revisa el stage antes de importar si hay dudas sobre nombres o duplicados
internos entre raíces.

## 5. Limpieza de fuentes externas

Solo después de que el álbum tenga copias verificadas y el usuario lo apruebe:

1. Archiva los JSON de la raíz:

   ```bash
   OUT=".bbcp/metadata-archive/takeout-2026.tar.gz"
   find "$ROOT" -type f -name '*.json' -printf '%P\0' \
     | tar -C "$ROOT" --null -T - -czf "$OUT.part"
   gzip -t "$OUT.part" && mv "$OUT.part" "$OUT"
   ```

2. Purga en cuarentena (recomendado):

   ```bash
   bbcp apply --execute --only external_covered --external-remove quarantine
   bbcp apply --execute --only external_dup --external-remove quarantine
   ```

3. Solo si se quiere liberar espacio de verdad y hay backup:
   `--external-remove unlink`. Es irreversible.

## 6. Google Fotos

```bash
bbcp plan      # imprescindible: google usa el plan para saber qué sobrevive
bbcp google
```

Salidas en `.bbcp/manifests/<fecha>/`:

* `google_delete_candidates.tsv` — columnas `google_id`, `title`, `taken`,
  `bytes`, `takeout_path`, `album_ref`, `match`.
* `google_batches/batch_NNN.tsv` — mismos ítems en trozos de `google.batch_size`.

Condiciones para que un ítem sea candidato: su sidecar está emparejado con un
archivo local, ese archivo pertenece a un grupo exacto con una copia del álbum
que sobrevive al plan, y la ruta de esa copia existe.

Nunca borres en Google un ítem que no esté en un lote. Los ítems sin copia
local cuentan como `kept`.

Estrategia: revisa el uso real de la cuenta, empieza por el lote de vídeos o
los más pesados, y borra por tandas comprobando el espacio después de cada
una.

## 7. Informe y cierre

```bash
bbcp report
```

Genera `.bbcp/manifests/<fecha>/report.md` con inventario, plan por tipo,
grupos, fechas conocidas y verificaciones. Guarda también los journals.

Al terminar:

```bash
bbcp scan --prune
bbcp status
du -sh /ruta/Album .bbcp/quarantine
```

## 8. Rollback

Cada journal TSV tiene `source`, `target`, `backup`, `status`.

* Duplicados del álbum:
  `mv .bbcp/quarantine/<fecha>/album/<ruta> /ruta/Album/<ruta>`
* Upgrades: la copia antigua está en
  `.bbcp/quarantine/<fecha>/upgrade/album/<ruta>`.
* Fuentes: `.bbcp/quarantine/<fecha>/source/<raíz>/<ruta>`.
* Stage: `.bbcp/staging/<fecha>/<raíz>/<ruta>`; vuelve a mover el archivo y
  actualiza la BD con `bbcp scan --root staging-<fecha>` o reescanea.

## 9. Diagnóstico

| Síntoma | Causa probable | Acción |
|---|---|---|
| `fechas` no rellena nada | `group` no se ejecutó antes | `bbcp group && bbcp dates` |
| Grupo gigante de imágenes | cierre transitivo o hashes degenerados | no existe cierre transitivo; sube `min_popcount` |
| Muchos `keep_distinct` | ráfagas o copias recomprimidas | revisar MAD con `bbcp compare A B` |
| Vídeos no agrupados | no comparten tamaño o falta `hash` | `bbcp hash --all` |
| `apply` dice "reference is not present" | plan obsoleto o referencia borrada | `bbcp scan --prune && bbcp plan` |
| Colisión en cuarentena | dos raíces con la misma ruta relativa | inspeccionar y resolver a mano |
| Google sin candidatos | sidecars sin emparejar o plan no ejecutado | `bbcp sidecars && bbcp plan` |
| Todo va lentísimo | BD o cuarentena en 9p/NFS | mover `.bbcp/` a disco local |

## 10. Checklist final

- [ ] `bbcp doctor` sin avisos.
- [ ] `scan`, `sidecars`, `probe`, `fingerprint`, `hash`, `group`, `dates`.
- [ ] `plan` revisado (conteos y bytes).
- [ ] `verify` ejecutado; `keep_distinct` revisados.
- [ ] Dry run de `apply` mostrado al usuario.
- [ ] `upgrade,import` ejecutados y verificados.
- [ ] Duplicados en cuarentena.
- [ ] Stage revisado e importado.
- [ ] JSON archivados antes de purgar fuentes.
- [ ] Lotes de Google revisados; borrado por tandas.
- [ ] `report` generado y journals conservados.
- [ ] Rescan final y espacio comprobado.
