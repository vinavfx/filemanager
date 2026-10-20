# Still Files

Gestor de archivos minimalista para Linux, escrito en Rust con egui, con Player integrado para imágenes, secuencias y vídeos.

## Ejecutables para Linux

Descarga el paquete `still-files-vX.Y.Z-linux-x86_64.tar.gz` desde [GitHub Releases](https://github.com/vinavfx/filemanager/releases/latest)
y extráelo. Abre `still-files` con doble clic. Desde `v0.1.3`, los binarios se compilan como ejecutables ELF no PIE para que los gestores de
archivos antiguos no los confundan con bibliotecas compartidas. Conserva sus permisos de ejecución al extraerlos; el escritorio debe
permitir ejecutar aplicaciones locales.

No necesitas Rust para usar los binarios. Mantén `still-files` y `player` en la misma carpeta. También puedes ejecutar `./still-files` desde
una terminal.

Desde `v0.1.2`, requiere Linux x86_64 con glibc 2.28 o posterior, como Rocky Linux 8, un escritorio X11 o Wayland y OpenGL. El paquete
`v0.1.1` requiere glibc 2.35 o posterior. Las herramientas opcionales se indican en «Arranque». Puedes verificar la descarga con
`sha256sum -c still-files-vX.Y.Z-linux-x86_64.tar.gz.sha256`.

La publicación es automática al subir una etiqueta nueva con formato `vX.Y.Z`: GitHub Actions compila ambos ejecutables en un contenedor
Rocky Linux 8 con Rust 1.89 y los archivos `Cargo.lock`, comprueba que no requieran una glibc superior a 2.28 y adjunta el paquete y su suma
SHA-256 al release. También puedes ejecutar el flujo «Linux release» manualmente desde GitHub Actions indicando una etiqueta existente. El
ejecutable es portable entre carpetas, no está enlazado estáticamente ni incluye todas las bibliotecas del sistema.

## Instalación en la carpeta de usuario

Ejecuta `./install.sh` desde el repositorio con ambos binarios de producción ya compilados, o desde el paquete de una release que incluya el
instalador. No compila, descarga dependencias ni requiere `sudo`.

Copia `still-files` y `player` juntos a `~/.local/bin/`, con permisos de ejecución, y añade esa carpeta al `PATH` en `~/.profile` y
`~/.bashrc`. Abre una terminal nueva y ejecuta `still-files /ruta/a/una/carpeta`. En otros intérpretes, configura también su `PATH` si es
necesario. NukeTool busca primero `~/.local/bin/still-files` y después el `PATH`, sin depender de la ubicación del repositorio.

Puedes mover o eliminar la carpeta original después de instalar. Para actualizar, vuelve a ejecutar `./install.sh` con los binarios nuevos y
cierra los procesos residentes de ambas aplicaciones antes de abrirlas otra vez. Para desinstalar, elimina `~/.local/bin/still-files` y
`~/.local/bin/player`; las preferencias se conservan.

## Bloques independientes

| Bloque                     | Fuentes y manifiesto                | Ejecutable compilado                       |
| -------------------------- | ----------------------------------- | ------------------------------------------ |
| Filemanager                | `app/src/` y `app/Cargo.toml`       | `./app/target/release/still-files`         |
| [Player](player/README.md) | `player/src/` y `player/Cargo.toml` | `./player/target/release/player [archivo]` |

Cada bloque conserva sus dependencias, `Cargo.lock`, compilaciones en `target/` y proceso residente. No comparten módulos ni requieren el
repositorio externo de player. Filemanager abre los medios con el ejecutable `player`; los demás archivos usan su aplicación asociada.

Para compilar ambos sin descargar dependencias:

```bash
(cd app && cargo build --offline)
(cd player && cargo build --offline)
```

## Arranque

Para compilar desde el código fuente se requiere Rust 1.89 o superior. La aplicación necesita un escritorio X11 o Wayland y OpenGL.
Herramientas utilizadas:

| Herramienta              | Función                                                                  |
| ------------------------ | ------------------------------------------------------------------------ |
| `xdg-open`               | Abrir archivos con la aplicación externa asociada y ubicaciones remotas. |
| `gio`                    | Operaciones de papelera y consulta de iconos MIME.                       |
| Konsole                  | Abrir una terminal desde el menú contextual.                             |
| `fzf`                    | Búsqueda recursiva; debe estar en `PATH` o `~/.fzf/bin/fzf`.             |
| `zip`                    | Comprimir archivos y carpetas.                                           |
| `ffmpeg` y `ffprobe`     | Crear previsualizaciones y decodificar medios en Player.                 |
| `gdk-pixbuf-thumbnailer` | Rasterizar iconos SVG/XPM del tema del sistema.                          |
| `date`                   | Mostrar fechas en la zona horaria local.                                 |

Desde la raíz del repositorio:

```bash
./app/target/release/still-files
./app/target/release/still-files /ruta/a/una/carpeta
```

Compila antes con `cd app && cargo build --release --offline`. Si está instalado, también puedes ejecutar `still-files` desde cualquier
carpeta. La interfaz está en inglés.

La primera apertura deja un proceso residente por sesión gráfica. Las siguientes abren ventanas en ese mismo proceso, sin reiniciar el motor
gráfico. Al cerrar todas las ventanas, permanece en segundo plano para acelerar la reapertura; el contenido y las miniaturas de cada carpeta
siguen cargándose según sea necesario. Para terminar también el proceso residente, ejecuta `still-files --quit`. Después de recompilar, usa
esa orden antes de volver a abrir la aplicación para cargar el ejecutable nuevo.

## Atajos de teclado

`Ctrl+N` significa pulsar ambas teclas juntas. `gg`, `yy`, `dd`, `zz` y `gt` son secuencias: pulsa una tecla y después la otra. `G`, `O` y
`T` mayúsculas requieren `Mayús`. Los atajos de archivos no actúan mientras escribes en un campo de texto.

### Vistas y visualización

| Atajo                 | Acción                             | Detalle                                                                                                                                      |
| --------------------- | ---------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| `m`                   | Alternar vista multimedia          | Cambia entre `Media` y la vista anterior, sin cambiar el modo recursivo.                                                                     |
| `t`                   | Mostrar árbol                      | Activa `Tree`, con carpetas desplegables y columnas.                                                                                         |
| `Ctrl+G`              | Mostrar cuadrícula                 | Activa `Grid`, con iconos de todos los archivos.                                                                                             |
| `r`                   | Alternar multimedia recursiva      | Muestra imágenes, vídeos y secuencias de la carpeta actual y sus subcarpetas.                                                                |
| `s`                   | Agrupar o separar secuencias       | Alterna entre grupos de imágenes numeradas y archivos individuales.                                                                          |
| `Ctrl+H`              | Mostrar u ocultar archivos ocultos | Durante una búsqueda tiene otra función; consulta su tabla.                                                                                  |
| `Ctrl+Shift+R` / `F5` | Actualizar                         | Recarga el contenido, los recuentos y el espacio del disco sin perder las expansiones del árbol; en búsqueda vuelve a recorrer las carpetas. |
| `=` / `-`             | Aumentar / reducir tamaño          | Cambia el tamaño de iconos y miniaturas, no el de la letra.                                                                                  |
| `Ctrl+rueda`          | Ajustar tamaño                     | Alternativa con el ratón al ajuste de tamaño.                                                                                                |

### Movimiento y apertura

| Atajo               | Acción                            | Detalle                                                                                    |
| ------------------- | --------------------------------- | ------------------------------------------------------------------------------------------ |
| `↑` / `k`           | Mover arriba                      | Selecciona el elemento superior.                                                           |
| `↓` / `j`           | Mover abajo                       | Selecciona el elemento inferior.                                                           |
| `←` / `h`           | Mover a la izquierda              | En árbol, cierra la carpeta o selecciona su padre visible.                                 |
| `→` / `l`           | Mover a la derecha                | En árbol, abre la carpeta o selecciona su primer hijo.                                     |
| Número + movimiento | Repetir el movimiento             | Por ejemplo, `5j` baja cinco posiciones en el panel de archivos.                           |
| `Home` / `gg`       | Ir al principio                   | Selecciona el primer elemento.                                                             |
| `End` / `G`         | Ir al final                       | Selecciona el último elemento.                                                             |
| `Ctrl+D` / `Ctrl+U` | Bajar / subir media pantalla      | Desplaza la vista y la selección.                                                          |
| `zz`                | Centrar la selección              | Centra el elemento actual en el panel de archivos.                                         |
| `Enter` / `Espacio` | Abrir                             | Entra en carpetas; abre medios con Player y los demás archivos con su aplicación asociada. |
| `o`                 | Abrir o desplegar                 | En árbol alterna la apertura de la carpeta; en archivos equivale a abrir.                  |
| `Mayús+O`           | Desplegar o plegar recursivamente | Solo en árbol; mantiene abierta la carpeta seleccionada.                                   |
| `Alt+P`             | Seleccionar el padre visible      | Solo en árbol; no cambia de directorio ni pliega carpetas.                                 |

`Alt+O` abre el menú contextual del archivo seleccionado. Dentro del menú, `h/j/k/l`, flechas, `w/e/b`, números con movimiento, `Home/End`,
`gg/G` y `Ctrl+D/U` navegan entre las opciones habilitadas; `o`, `Enter` o `Espacio` ejecutan la opción y `Esc` cierra el menú sin quitar la
selección. También funciona al abrirlo con clic derecho.

### Navegación entre carpetas y paneles

| Atajo                       | Acción                       | Detalle                                                      |
| --------------------------- | ---------------------------- | ------------------------------------------------------------ |
| `Alt+←` / `Ctrl+O`          | Volver                       | Retrocede en el historial de carpetas.                       |
| `Alt+→` / `Ctrl+I`          | Avanzar                      | Avanza en el historial de carpetas.                          |
| `Alt+↑` / `gT`              | Subir al padre               | Cambia al directorio superior.                               |
| `gt`                        | Bajar por la ruta conservada | Avanza un nivel por los botones de ruta que siguen visibles. |
| `Ctrl+W`, después `h` o `←` | Enfocar marcadores           | Cambia al panel lateral.                                     |
| `Ctrl+W`, después `l` o `→` | Enfocar archivos             | Vuelve al panel de archivos.                                 |

En marcadores, `j/k`, `↑/↓`, `Home/End`, `gg/G` y `Ctrl+D/U` permiten navegar. `o`, `Enter` o `Espacio` abren el marcador.

### Selección y operaciones con archivos

| Atajo                                   | Acción                              | Detalle                                                                                                                              |
| --------------------------------------- | ----------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------ |
| `Mayús` + flechas o `h/j/k/l`           | Extender la selección               | Selecciona un rango desde el punto inicial.                                                                                          |
| `Ctrl+A`                                | Seleccionar todo lo visible         | En árbol incluye los hijos desplegados, no el contenido de carpetas cerradas.                                                        |
| `v` o `Shift+V`                         | Activar o salir de selección visual | Los movimientos amplían o reducen el rango; la barra inferior indica `VISUAL LINE`. No pega archivos.                                |
| `yy`                                    | Copiar la selección                 | Guarda los elementos en un registro interno; una sola `y` fuera del modo visual no copia.                                            |
| `y` en modo visual                      | Copiar y salir del modo visual      | Deja seleccionado el primer elemento del rango.                                                                                      |
| `p`                                     | Pegar                               | Inserta el grupo después del último seleccionado; las nuevas copias quedan seleccionadas.                                            |
| `dd` / `Supr`                           | Enviar a la papelera                | Sin confirmación; una sola `d` fuera del modo visual no borra.                                                                       |
| `d` en modo visual                      | Enviar el rango a la papelera       | Sin confirmación; se puede deshacer.                                                                                                 |
| `n`                                     | Renombrar                           | Edita el nombre en el propio elemento; también admite selección múltiple y secuencias.                                               |
| `Ctrl+N`                                | Crear una carpeta en línea          | Muestra una carpeta provisional: escribe el nombre y pulsa `Enter`; `Esc` la descarta sin crear nada en disco.                       |
| `u`                                     | Deshacer                            | Revierte la última operación compatible.                                                                                             |
| `Ctrl+R`                                | Rehacer                             | Repite la operación deshecha.                                                                                                        |
| `Enter` durante la edición de un nombre | Confirmar                           | Guarda el nombre o crea la carpeta provisional.                                                                                      |
| `Esc`                                   | Cancelar o quitar selección         | Cancela primero la edición de nombres; en otros contextos cierra avisos o campos activos, sale del modo visual o quita la selección. |

Al salir de selección visual con `Esc`, `v` o `Shift+V`, queda seleccionado el primer elemento del rango. En los campos de texto se
mantienen las funciones habituales de edición, como `Ctrl+A` para seleccionar texto y `Ctrl+V` para pegarlo.

### Búsqueda

| Atajo                          | Contexto                       | Acción                                                                                                  |
| ------------------------------ | ------------------------------ | ------------------------------------------------------------------------------------------------------- |
| `Ctrl+F`                       | Panel de archivos              | Busca por nombre entre los elementos visibles y selecciona la mejor coincidencia sin ocultar los demás. |
| `Ctrl+P`                       | Fuera de la búsqueda recursiva | Abre la búsqueda de nombres y rutas en la carpeta y sus subcarpetas.                                    |
| `Enter` / `Esc`                | Búsqueda de elementos visibles | Cierra el campo conservando la selección, sin abrir el archivo.                                         |
| `↑` / `↓`                      | Búsqueda recursiva             | Selecciona el resultado anterior / siguiente.                                                           |
| `Ctrl+H` / `Ctrl+K` / `Ctrl+P` | Búsqueda recursiva             | Selecciona el resultado anterior sin cambiar la consulta.                                               |
| `Ctrl+J` / `Ctrl+L` / `Ctrl+N` | Búsqueda recursiva             | Selecciona el resultado siguiente sin cambiar la consulta.                                              |
| `Enter`                        | Búsqueda recursiva             | Sale de la búsqueda y muestra el resultado seleccionado en su carpeta.                                  |
| `Esc`                          | Búsqueda recursiva             | Cierra la búsqueda y restaura la dirección.                                                             |

**Durante la búsqueda recursiva, `Ctrl+N` no crea carpetas, `Ctrl+H` no alterna los ocultos y `Ctrl+L` no edita la dirección.** Las letras
sin modificadores sirven para escribir la consulta. En la papelera, `Ctrl+P` utiliza la búsqueda de elementos visibles.

### Edición de la ruta y equivalencias

| Atajo               | Contexto                        | Acción                                                              |
| ------------------- | ------------------------------- | ------------------------------------------------------------------- |
| `Ctrl+L`            | Fuera de la búsqueda recursiva  | Alterna entre los botones de ruta y la dirección editable.          |
| `Tab` / `Mayús+Tab` | Dirección editable              | Completa y recorre las carpetas coincidentes hacia delante / atrás. |
| `Ctrl+E`            | Cursor al final de la dirección | Acepta el autocompletado propuesto.                                 |
| `Ctrl+W`            | Dirección editable              | Borra el último componente de la ruta; aquí no cambia de panel.     |
| `Enter`             | Dirección editable              | Abre la ruta escrita; admite rutas relativas y `~`.                 |
| `Esc`               | Dirección editable              | Cancela la edición.                                                 |
| `Ctrl+M`            | General                         | Equivale a `Enter`.                                                 |
| `Ctrl+[`            | General                         | Equivale a `Esc`.                                                   |

## Comportamiento de archivos y carpetas

### Copiar, mover y renombrar

Con `Ctrl+N`, la carpeta provisional utiliza el mismo editor en línea que el renombrado. En árbol, si la carpeta seleccionada está
expandida, se crea dentro de ella, antes de sus hijos. Si está cerrada o hay un archivo seleccionado, se crea debajo como hermana, en el
mismo directorio. `Enter` confirma y `Esc` descarta la carpeta provisional sin escribir en disco.

- Un clic selecciona; doble clic abre. Arrastra archivos o carpetas sobre otra carpeta para moverlos, también con selección múltiple y
  secuencias. Arrastrar una carpeta a la barra de marcadores añade un acceso, no la mueve.
- El pegado copia archivos, carpetas, enlaces simbólicos y los fotogramas de las secuencias. No sobrescribe: añade ` (copy)` y un número
  cuando el nombre ya existe. Cada nueva pulsación de `p` coloca el grupo debajo del anterior.
- En árbol, pegar sobre una carpeta abierta coloca las copias dentro, antes de sus hijos; sobre una cerrada las coloca como hermanas. Sin
  selección, pega al principio de la carpeta actual. El orden se conserva al reiniciar; pulsar un encabezado restaura la ordenación.
- Al renombrar varios elementos, todos muestran el mismo nombre mientras escribes. Al confirmar, los archivos reciben números antes de la
  extensión: `foto.jpg` produce `foto_1.jpg`, `foto_2.jpg`, etc. La extensión forma parte del texto editable, no se recupera
  automáticamente.
- El renombrado de secuencias conserva el número, los ceros iniciales y el separador de cada fotograma. Se validan los destinos antes de
  empezar, sin sobrescribir. Copiar y renombrar reutilizan las miniaturas existentes.
- Deshacer y rehacer conservan la selección cuando es posible; si desaparece, eligen el elemento superior más cercano, o el siguiente si no
  hay ninguno arriba. Si falla el envío a la papelera, no se sustituye por un borrado permanente. Vaciar la papelera pide confirmación y es
  irreversible.
- El menú contextual permite crear carpetas, abrir Konsole, copiar rutas, añadir marcadores y comprimir a ZIP en segundo plano. Los ZIP
  quedan junto al original, sin sobrescribir, y conservan los enlaces simbólicos como enlaces.

### Vistas y multimedia

| Vista o control        | Comportamiento                                                                                                                                                                                                              |
| ---------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `Tree`                 | Carpetas expandibles y columnas `Name`, `Size` y `Date Modified`; las carpetas permanecen primero. Pulsar un encabezado ordena cada nivel y vuelve a pulsarlo para invertir el sentido.                                     |
| `Grid`                 | Todos los archivos y carpetas con iconos y nombres.                                                                                                                                                                         |
| `Media`                | Mosaico de imágenes, vídeos y carpetas; solo las carpetas muestran su nombre. La selección usa una capa de color semitransparente.                                                                                          |
| `Recursive media`      | Solo imágenes, vídeos y secuencias de la carpeta actual y sus subcarpetas, en cualquiera de las tres vistas. Respeta ocultos y agrupación; no sigue enlaces simbólicos a carpetas. Se desactiva al abrir una ventana nueva. |
| `Sequences` / `Images` | Agrupa imágenes numeradas con la misma base y extensión, aunque haya saltos. Muestra los intervalos existentes y el total; doble clic abre el primer fotograma.                                                             |
| `Light` / `Dark`       | Alterna el tema y conserva la elección al reiniciar.                                                                                                                                                                        |

En árbol, el tamaño de una carpeta es su número de elementos inmediatos, incluidos los ocultos; en archivos es el tamaño en bytes. Las
secuencias muestran la fecha del fotograma más reciente. Las carpetas con contenido muestran dos hojas en el icono; `F5` actualiza el
recuento.

La expansión recursiva del árbol no sigue enlaces simbólicos y respeta la visibilidad de ocultos. Por operación se limita a 5 niveles, 64
carpetas, 2000 entradas examinadas y un presupuesto de 250 ms comprobado entre lecturas. Si alcanza un límite, avisa; `o` permite continuar
manualmente.

Los vídeos muestran una imagen central. Mueve el cursor horizontalmente sobre vídeos o secuencias para explorar hasta ocho muestras, sin
reproducción continua; la barra inferior indica la posición. Los archivos se abren con su aplicación externa, no con un visor integrado.

### Búsqueda y dirección

La búsqueda visible ignora mayúsculas y prioriza nombres exactos, prefijos y fragmentos contiguos; también admite letras en orden con
caracteres intermedios. En árbol solo incluye las carpetas desplegadas.

La búsqueda recursiva utiliza fzf sin mostrar su interfaz. Admite `'texto` para coincidencia literal, `^texto` para prefijo, `texto$` para
sufijo, `!texto` para excluir y `|` para alternativas. Los términos separados por espacios se combinan. No carga `FZF_DEFAULT_OPTS` ni
`FZF_DEFAULT_OPTS_FILE`.

El recorrido y las consultas se ejecutan en segundo plano; las consultas anteriores se cancelan. Se muestran los mejores 1000 resultados y
el número total de coincidencias. La columna `Location` indica la carpeta absoluta. Se respeta la visibilidad de ocultos, no se recorren
enlaces simbólicos y se indican ubicaciones inaccesibles. Los resultados son archivos individuales, aunque esté activa la agrupación de
secuencias. El índice se reutiliza mientras la búsqueda permanece abierta; el tiempo total incluye el recorrido, fzf y la interfaz.

Mostrar un resultado con `Enter` desactiva la multimedia recursiva y selecciona el archivo o su secuencia. Si no es multimedia y la vista
era `Media`, cambia a `Grid`. El doble clic abre el resultado directamente.

La dirección sugiere carpetas y enlaces a carpetas; para nombres que empiezan por punto debes escribirlo o activar los ocultos. Al pulsar un
directorio anterior, se conservan los botones de ruta siguientes hasta abrir otra ruta.

## Marcadores, estado y almacenamiento

Los marcadores se añaden con `+`, desde el menú contextual o arrastrando carpetas desde `Grid` o `Tree`. Los accesos principales utilizan
rutas XDG; quitarlos desde su menú contextual no borra las carpetas. El menú `...` permite restaurarlos.

La barra de estado muestra espacio usado y total sin selección, o el tamaño del archivo y la suma recursiva de la carpeta seleccionada. El
cálculo incluye ocultos, no sigue enlaces simbólicos, se ejecuta en segundo plano y señala resultados parciales. Los tamaños son lógicos, no
bloques ocupados.

| Datos                           | Ubicación y comportamiento                                                                                                                                                                                   |
| ------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Preferencias                    | `$XDG_CONFIG_HOME/still-files/settings.json` o `~/.config/still-files/settings.json`. Conserva vista, zoom, tema, secuencias, ocultos, accesos predeterminados ocultos y orden de pegado.                    |
| Marcadores compartidos con Caja | `$XDG_CONFIG_HOME/gtk-3.0/bookmarks` o `~/.config/gtk-3.0/bookmarks`; si no existe, lee `~/.gtk-bookmarks`. Una URI por línea y un nombre opcional. Detecta cambios cada segundo y conserva nombres y orden. |
| Miniaturas                      | `$XDG_CACHE_HOME/still-files/thumbnails` o `~/.cache/still-files/thumbnails`. WebP de hasta 192 píxeles por lado; se reutilizan entre ejecuciones y se invalidan por tamaño o fecha del original.            |

Los marcadores antiguos de `settings.json` se ignoran. Los marcadores locales inexistentes y los duplicados de accesos predeterminados se
ocultan sin borrarlos del archivo; su disponibilidad se comprueba cada segundo. Las ubicaciones remotas se abren mediante `xdg-open`.

## Miniaturas y formatos

- Imágenes: PNG, JPEG, GIF —primer fotograma—, WebP, BMP, TIFF, ICO y EXR. Los EXR se convierten de RGB lineal a sRGB, sin mapeo tonal y
  recortando al rango de pantalla; el visualizador externo también debe admitirlos. La decodificación se limita a 256 MiB.
- Vídeos: MPG/MPEG, MP4, MOV —incluido ProRes—, MKV, AVI, WebM, MXF y otros según los decodificadores de FFmpeg instalados. Se guardan hasta
  ocho muestras por vídeo: primero la central y después las posiciones exploradas.
- Dos trabajadores y hasta 512 miniaturas en memoria, compartidas con las imágenes. Las tareas de vídeo se interrumpen al cambiar de carpeta
  o superar 15 segundos; los archivos dañados o incompatibles muestran un icono genérico.
- La barra inferior muestra el progreso. Al cambiar de carpeta se cancelan solicitudes pendientes; una decodificación en curso puede
  terminar, pero su resultado se descarta sin guardarlo. Las miniaturas ya terminadas permanecen disponibles.
- Los archivos sin miniatura utilizan iconos MIME del tema activo, detectado al iniciar. Se cargan en segundo plano y se reutilizan por
  extensión; si falta una herramienta o un icono, se utiliza el genérico.

## Verificación

Desde la raíz:

```bash
cd app
cargo fmt --all -- --check
cargo clippy --offline --all-targets --all-features -- -D warnings
cargo test --offline
cargo build --offline
```

Las órdenes con `--offline` requieren que las dependencias ya estén disponibles localmente.
