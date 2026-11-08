# Playbook operativo

Guía completa para consolidar una biblioteca real con `bbcp`. Está pensada para
que la siga una persona o un agente sin conocimiento previo del proyecto.

Si solo quieres probar la herramienta, empieza por el
[README](../README.md#quickstart). Aquí se documenta una ejecución seria.

## 0. Objetivo y principios

Consolidar en un único álbum local todas las fotos y vídeos dispersos en
exports de Google Photos, volcados antiguos y papeleras, quedándose con la
**mejor copia** de cada contenido, y preparar el borrado por lotes en Google
Fotos sin perder nada.

Principios:

1. **Nada se borra sin una copia verificada.**
2. **Todo es reversible.** Los borrados van a cuarentena en el mismo sistema
   de archivos (rename atómico) y quedan en un journal.
3. **No asumir que el álbum es la mejor copia.** Puede haber versiones de más
   resolución en carpetas temporales.
4. **Registrar antes de actuar.** Cada fase deja manifiestos TSV y journals.
5. **Detener la sincronización** (ownCloud, Syncthing, Drive) mientras se
   mueven archivos del álbum.
6. **La base de datos vive en un sistema de archivos local**, nunca en 9p,
   NFS o FUSE.

## 1. Entorno

Ejemplo de disposición (adapta las rutas):

| Elemento | Ruta |
|---|---|
| Álbum | `/mnt/b/ownCloud/Album` |
| Takeout nuevo | `/mnt/b/GPhotos/Takeout/Takeout/Google Fotos` |
| Cuarentena | `/mnt/b/PMM/quarantine` |
| Base de datos | `/home/juampe/.local/share/bbcp/bbcp.db` |
| Logs / journals | `/mnt/b/PMM/logs` |
| Manifiestos | `/mnt/b/PMM/manifests/<YYYY-MM-DD>` |
| Archivo de metadatos | `/mnt/b/PMM/metadata-archive` |
| Stage de revisión | `/mnt/b/PMM/staging-revision/<YYYY-MM-DD>` |
| Configuración | `/home/juampe/Develop/byebyecp/bbcp.toml` |

Dependencias: `ffmpeg`, `ffprobe` y una base de datos local. Nada más.

El montaje `/mnt/b` puede ser 9p: cada operación de metadatos tiene latencia
alta. Por eso:

* la BD va en ext4;
* los hashes y fingerprints se paralelizan;
* el journal sincroniza cada 100 líneas, no por archivo;
* los renames/borrados se ejecutan en paralelo.

## 2. Arquitectura

### 2.1 Base de datos

`.bbcp/bbcp.db` (SQLite, WAL) contiene:

* **roots** — raíces inventariadas (`album`, `takeout-2026`, `old-staging`,
  `album-dtrash`, `todo`, `sync-delete`, `gdrive-delete`…).
* **files** — un registro por archivo: `path` (único), `rel_path`, `media`
  (`image`/`video`/`other`), `size`, `mtime`, `width`, `height`, `duration`,
  `bitrate`, `codec`, `sha256`, `phash`, `dhash`, `ahash`, `capture_ts`,
  `capture_source`, `fingerprint_status`, `meta_status`.
* **sidecars** — JSON de Takeout: `google_id`, `title`, `taken_ts`,
  `creation_ts`, `description`, `matched_file_id`.
* **groups** / **group_members** — grupos exactos; `kind` =
  `image-exact`/`video-exact`; `role` = `best`/`duplicate`.
* **plan_actions** — acción por archivo: `keep`, `keep_distinct`, `delete`,
  `upgrade`, `import`, `external_covered`, `external_dup`, con `reference_id`,
  `target_path`, `bytes`, `mad`, `reason`.

### 2.2 Módulos

| Módulo | Función |
|---|---|
| `walk` | inventario de raíces |
| `dates` | sidecars, puente entre copias, EXIF, nombre, carpeta |
| `video` | ffprobe (resolución con rotación, duración, bitrate) |
| `imagehash` | pHash/dHash/aHash, MAD, fotograma de vídeo |
| `hash` | SHA-256 en streaming |
| `group` | grupos exactos |
| `quality` | ranking y mejora material |
| `plan` | `plan_actions` |
| `verify` | MAD antes de borrar imágenes |
| `apply` | copias atómicas, cuarentena, journal |
| `stage` | stage de únicos |
| `google` | candidatos y lotes de Google |
| `report` | informe Markdown |

## 3. Reglas de decisión

### 3.1 Imágenes: duplicado exacto perceptual

Clave: terna `(pHash, dHash, aHash)` de 64 bits.

Guardas obligatorias:

* `popcount` de **cada** hash ≥ 12. Descarta imágenes lisas, negras o capturas
  que colisionan.
* Variación de **aspect ratio** dentro del grupo ≤ 5 %. Si no, se descarta el
  grupo.
* **Sin cierre transitivo.** No se encadenan grupos: una ráfaga puede acabar
  con cientos de fotos distintas unidas.
* Verificación final por píxeles (MAD) antes de borrar.

**MAD** = diferencia absoluta media de dos miniaturas 64×64 en gris.

| MAD | Interpretación | Acción |
|---|---|---|
| 0 – 0,5 | copias idénticas | borrar la peor |
| 0,5 – 1,2 | recomprimidas | borrar la peor |
| ≥ 2,7 | frames distintos de ráfaga | conservar ambas |

Umbral por defecto: `mad_threshold = 1.5`. Por encima, la acción se convierte
en `keep_distinct`.

### 3.2 Vídeos: duplicado exacto y approx

* **Exacto**: mismo SHA-256. Solo se hashean los candidatos que comparten
  tamaño con otro vídeo (ahorra leer cientos de GB).
* **Approx** (mismo contenido, distinto encoding): misma resolución y
  `|Δduración| ≤ 0,5 s`, y verificación por fotograma: extraer un frame en el
  punto medio y comparar `pHash + dHash`.
  * distancia ≤ 16 → mismo vídeo;
  * distancia > 16 → vídeos distintos (no borrar).

No fiarse solo de duración+resolución: en una biblioteca real, ~85 % de los
"approx" eran vídeos distintos.

La implementación actual agrupa vídeos por SHA-256. La comprobación por
fotograma (`frame_distance`) queda como herramienta de diagnóstico
(`bbcp compare` para imágenes; la extracción de frames está en
`imagehash::video_frame`) y como base para extensiones futuras.

### 3.3 Ranking de calidad

Orden: `(píxeles, bitrate o tamaño, antigüedad, tamaño)`.

* Los píxeles mandan: una copia de 12 MP gana a una de 2 MP.
* A igual resolución, gana la que tiene más bitrate (vídeo) o más bytes
  (imagen menos comprimida).
* A igualdad, gana la más antigua (las derivadas se escriben después).
* El tamaño es el desempate final.

**Mejora material** (para sustituir): más resolución, o misma resolución con
bitrate/tamaño > 1,25×. No se sustituye por un 2 % de diferencia.

### 3.4 Fechas

Orden de fuentes:

1. `sidecar` del propio archivo (`photoTakenTime`).
2. `sidecar` de una **copia exacta** en otra raíz (puente de metadatos).
3. EXIF `DateTimeOriginal` (o digitized/created).
4. Nombre de archivo (`2021-08-07 08.18.41`, `IMG_20210807_081841`, …).
5. Carpeta `YYYY/YYYY-MM-DD`.

Exclusiones:

* Centinelas `1970-01-01` y `1980-01-01`.
* Carpetas de volcado masivo configuradas en `import_dump_days`: la fecha del
  nombre/carpeta es la de **importación**, no la de captura.

## 4. Flujo completo

### Fase 0 — Preflight

```bash
./scripts/preflight.sh
bbcp doctor
# ownCloud/Syncthing/Drive parados
df -h /mnt/b | tail -1
sqlite3 .bbcp/bbcp.db "PRAGMA journal_mode;"   # wal
```

### Fase 1 — Inventario

```bash
bbcp scan
# o por raíz:
bbcp scan --root album
bbcp scan --root takeout-2026
```

### Fase 2 — Sidecars de Google

```bash
bbcp sidecars
```

Empareja `x.jpg.json` y `x.jpg.supplemental-metadata.json` con `x.jpg`,
incluidos nombres truncados por Takeout antiguos (coincidencia única por
prefijo).

### Fase 3 — Metadatos

```bash
bbcp probe --workers 16
```

Solo vídeos. Las imágenes obtienen dimensiones en `fingerprint`.

### Fase 4 — Fingerprint de imágenes

```bash
bbcp fingerprint --workers 16
```

### Fase 5 — SHA-256 de vídeos candidatos

```bash
bbcp hash --workers 16
# todos:
bbcp hash --all --workers 16
```

Solo hashea vídeos que comparten tamaño con otro. Los de tamaño único no se
pueden confirmar por SHA-256.

### Fase 6 — Agrupar duplicados

```bash
bbcp group --reset
```

### Fase 7 — Fechas

```bash
bbcp dates
```

Debe ejecutarse **después** de `group`.

### Fase 8 — Plan

```bash
bbcp plan
```

Genera `keep`, `keep_distinct`, `delete`, `upgrade`, `import`,
`external_covered`, `external_dup` en la BD. No toca el disco.

### Fase 9 — Verificación

```bash
bbcp verify --workers 8
```

Convierte en `keep_distinct` los borrados cuya MAD supera el umbral o que no se
pueden decodificar.

### Fase 10 — Actualizar e importar (aditivo)

```bash
bbcp apply                                      # dry run
bbcp apply --execute --only upgrade,import
```

* Copia atómica: temporal en el mismo directorio, `fsync`, `os.replace`.
* Verifica SHA-256 origen/destino.
* El archivo antiguo del destino se mueve a
  `quarantine/<fecha>/upgrade/album/`.
* Journal reanudable en `.bbcp/logs/apply-<fecha>.tsv`.
* La base de datos se actualiza: un `import` pasa a apuntar a su ruta en el
  álbum, y una fila movida a cuarentena o borrada desaparece del inventario.

### Fase 11 — Borrar duplicados (a cuarentena)

```bash
bbcp apply --execute --only delete --album-remove quarantine --workers 32
```

Renombrados en paralelo a `quarantine/<fecha>/album/<ruta relativa>`. No borra.

### Fase 12 — Informe

```bash
bbcp report
```

### Fase 13 — Limpiar raíces externas

Antes de borrar media, **archivar los JSON** de metadatos:

```bash
mkdir -p .bbcp/metadata-archive
ROOT=/ruta/Takeout/Google\ Fotos
find "$ROOT" -type f -name '*.json' -printf '%P\0' \
  | tar -C "$ROOT" --null -T - -czf ".bbcp/metadata-archive/takeout.tar.gz.part"
gzip -t .bbcp/metadata-archive/takeout.tar.gz.part \
  && mv .bbcp/metadata-archive/takeout.tar.gz.part \
        .bbcp/metadata-archive/takeout.tar.gz
```

Después, purgar solo los `external_covered` (verificando que la copia del
álbum existe):

```bash
bbcp apply --only external_covered --external-remove quarantine
bbcp apply --execute --only external_covered --external-remove quarantine
```

Y los `external_dup`:

```bash
bbcp apply --execute --only external_dup --external-remove quarantine
```

`unlink` libera espacio de inmediato pero es irreversible. Úsalo solo con
backup y confirmación.

### Fase 14 — Papelera digiKam (`.dtrash`)

1. Añadir `.dtrash/files` como raíz `source` (label `album-dtrash`).
2. `bbcp scan --root album-dtrash`.
3. `bbcp fingerprint`, `bbcp hash`, `bbcp group --reset`, `bbcp plan`.
4. Borrar **solo** los duplicados que tengan copia exacta en el álbum:
   `bbcp apply --execute --only external_covered --external-remove quarantine`.
5. Conservar el resto y revisarlo a mano.

### Fase 15 — Raíces extra (TODO, Sync-DELETE, Drive-DELETE…)

Mismo pipeline (`scan → probe → fingerprint → hash → group → dates → plan`) y
después:

```bash
bbcp plan
# categorías:
#  album_dup   -> external_covered (borrar)
#  album_better-> upgrade (incorporar antes de borrar)
#  root_dup    -> external_dup (conservar la mejor)
#  unique      -> import / stage
#  other       -> no tocar
bbcp apply --execute --only upgrade,import
bbcp stage --execute
bbcp apply --execute --only import
bbcp apply --execute --only external_covered,external_dup --external-remove quarantine
```

### Fase 16 — Lotes de Google Fotos

```bash
bbcp plan                 # google usa el plan
bbcp google
```

Salidas:

* `google_delete_candidates.tsv` — ítems con copia segura en el álbum.
* `google_batches/batch_NNN.tsv` — trozos de `google.batch_size`.

Estrategia para bajar de ~148 GB a ~50 GB: revisar el uso real de la cuenta,
empezar por los vídeos y los más pesados, y borrar por tandas comprobando el
espacio después de cada una.

### Fase 17 — Stage de revisión

```bash
bbcp stage                       # dry run
bbcp stage --execute             # mover a .bbcp/staging/<fecha>
bbcp stage --list
bbcp apply --execute --only import
```

El stage detecta duplicados **entre** raíces: la misma foto puede estar en TODO
y en Sync-DELETE. `group` ya los une; el stage solo mueve los `import`.

### Fase 18 — Rescan final

```bash
bbcp scan --prune
bbcp status
bbcp report
du -sh /ruta/Album .bbcp/quarantine
```

## 5. Trampas conocidas

### 9p / rendimiento

* SQLite en 9p es lentísimo: la DB va en ext4.
* `fsync` por archivo mata el rendimiento: journal en lote (cada 100 líneas) y
  renames/deletes en paralelo.
* `find | tar` de miles de JSON pequeños es lento; escribe a `.part` y renombra
  solo tras `gzip -t`.

### Fingerprints

* Hashes degenerados (todo ceros / un bit) provocan colisiones masivas. Filtro
  `popcount ≥ 12`.
* El cierre transitivo une ráfagas y fotos distintas. **Nunca** usarlo.
* Diferencias de resolución entre copias de la misma foto son normales; el
  aspect ratio debe coincidir.

### SQLite

* Una conexión no se comparte entre hilos: resuelve rutas/IDs en el hilo
  principal antes de paralelizar.
* Tras mover/borrar archivos, la BD queda obsoleta: reescanea o usa el journal
  como fuente de verdad.

### Vídeos

* Mismo tamaño no implica mismo contenido: confirmar con SHA-256.
* Misma duración+resolución **no** implica mismo vídeo: verificar por
  fotograma.
* Los `.nsp`, ROMs y documentos no son fotos/vídeos: no tocarlos.

### Fechas

* `1970-01-01` y `1980-01-01` son centinelas.
* La carpeta `2026/2026-01-05` es un volcado de importación.
* El puente de sidecars vía copia exacta recupera miles de fechas perdidas.

### Operaciones

* Deduplicar **por destino** en los upgrades: dos orígenes pueden apuntar al
  mismo archivo del álbum y pisarse. `apply` aborta si el destino existe con
  contenido distinto.
* Verificar SHA-256 en cada copia; nunca confiar solo en el tamaño.
* Mover a cuarentena, no borrar, hasta la verificación final.
* `du -sb` incluye `.dtrash`, `Thumbs.db` y `.tmp`; la suma de la BD puede no
  coincidir.

## 6. Cifras de referencia (una ejecución real)

| Concepto | Valor |
|---|---|
| Imágenes inventariadas | ~317.000 |
| Vídeos | ~16.700 |
| Álbum inicial | 389,9 GB (138.490 archivos) |
| Duplicados del álbum a cuarentena | 42.729 (65,2 GB) |
| Imágenes confirmadas por MAD ≤ 1,5 | 41.327 (22,98 GB) |
| Conservadas por MAD > 1,5 | 1.039 |
| Reemplazos de calidad | 311 |
| Importaciones al álbum | 3.425 |
| Externos cubiertos | 260,34 GB |
| `.dtrash` confirmado | 31.462 (22,22 GB) |
| Raíces extra procesadas | 339 GB |
| Duplicados extra borrados | 130.994 (275,96 GB) |
| Stage de únicos | 17.783 (45,69 GB) |
| Google candidatos seguros | 86.485 (125,71 GB) |
| Google lote heavy | 9.930 (100 GB) |
| Google solo vídeos | 2.386 (83,60 GB) |

Son referencias, no garantías: los números dependen de cada biblioteca.

## 7. Rollback

Cada journal tiene columnas `source`, `target`, `backup`, `status`.

* Duplicados del álbum:
  `quarantine/<fecha>/album/<ruta relativa>`.
* Upgrades: `quarantine/<fecha>/upgrade/album/<ruta relativa>`.
* Fuentes: `quarantine/<fecha>/source/<raíz>/<ruta relativa>`.
* Stage: mover de vuelta desde `.bbcp/staging/<fecha>/<raíz>/<ruta>` y
  reescanear.

```bash
mv ".bbcp/quarantine/2026-11-08/album/2024/2024-05-01/x.jpg" \
   "/mnt/b/ownCloud/Album/2024/2024-05-01/x.jpg"
```

## 8. Reglas de seguridad para el agente

1. Antes de borrar, comprobar que la referencia del álbum existe.
2. No borrar `external_*` ni `keep` sin autorización explícita.
3. No borrar en Google ítems sin `album_ref` verificado.
4. No tocar documentos ni ROMs.
5. No reiniciar la sincronización hasta que el usuario lo pida.
6. Ante duda, conservar y marcar para revisión.
7. Escribir siempre journal y manifiesto antes de ejecutar.
8. Simular y revisar el resumen antes de ejecutar.

## 9. Checklist rápida

- [ ] Sincronización parada.
- [ ] BD en ext4 y en WAL.
- [ ] Raíces registradas y escaneadas.
- [ ] Sidecars parseados.
- [ ] Vídeos con ffprobe; imágenes con fingerprint.
- [ ] SHA-256 de vídeos candidatos.
- [ ] Grupos exactos (sin transitividad, con filtros).
- [ ] Fechas rellenadas (sin centinelas).
- [ ] Plan generado y revisado.
- [ ] MAD verificado.
- [ ] Upgrades/imports ejecutados con SHA-256.
- [ ] Duplicados a cuarentena.
- [ ] JSON archivados antes de borrar externos.
- [ ] `.dtrash` verificado por contenido.
- [ ] Raíces extra clasificadas; mejores incorporadas; únicos en stage.
- [ ] Lotes de Google generados y revisados.
- [ ] Informe y journals guardados.
- [ ] Rescan final.

## 10. Glosario

* **pHash/dHash/aHash** — hashes perceptuales de 64 bits; robustos a
  recompresión y cambio de tamaño.
* **MAD** — diferencia absoluta media entre miniaturas; mide si dos imágenes
  son el mismo contenido.
* **Approx** — vídeo con misma duración y resolución pero distinto SHA-256;
  requiere verificación por fotograma.
* **Stage** — carpeta intermedia con los archivos únicos para revisarlos antes
  de importarlos al álbum.
* **Cuarentena** — destino reversible de los borrados; mismo sistema de
  archivos.
* **Journal** — registro TSV de cada operación, con estado y rutas; permite
  reanudar.
* **Manifiesto** — listado TSV del plan (qué se conserva, borra, importa o
  actualiza).
