# Still Files

Gestor de archivos minimalista para Linux, escrito en Rust con egui. Todo está en este repositorio: la raíz contiene únicamente el lanzador
`start.sh` y `app/` contiene el proyecto.

## Arranque

Requiere Rust 1.89 o superior, un escritorio X11 o Wayland y OpenGL. Para abrir otros archivos utiliza `xdg-open`; la opción de terminal
requiere Konsole. La compresión utiliza `zip`. Las previsualizaciones de vídeo requieren `ffmpeg` y `ffprobe`.

Desde la raíz:

```bash
./start.sh
./start.sh /ruta/a/una/carpeta
```

El lanzador descarga las dependencias y compila si es necesario. Las siguientes ejecuciones reutilizan la compilación. La interfaz está en
inglés.

## Uso

- Un clic selecciona; doble clic o `Enter` abre.
- `Light`/`Dark`, en la barra superior, alterna entre los temas claro y oscuro; la elección se conserva al reiniciar.
- Arrastra una carpeta desde `Grid` o `Tree` a la barra de marcadores, o selecciónala y pulsa `D` o `Ctrl+D`.
- Los accesos a la carpeta personal y sus carpetas principales aparecen arriba, con iconos propios y rutas XDG. Clic derecho permite
  quitarlos sin borrar carpetas; el menú `...` restaura los accesos predeterminados.
- `Tree` muestra carpetas expandibles y `Grid` muestra todos los archivos con iconos. `Media` muestra solo imágenes y vídeos en un mosaico
  sin separación, recortados al centro para llenar cada celda; el nombre aparece al pasar el cursor o seleccionar. Las tres vistas cargan
  miniaturas en segundo plano. Los vídeos muestran una imagen del centro; mueve el cursor de izquierda a derecha para recorrer muestras
  desde el inicio hasta el final, sin reproducción continua. El zoom cambia el tamaño de las miniaturas y los iconos, no el de la letra.
- `Ctrl+L` o `Edit` activa la dirección editable; `Path` vuelve a los botones de ruta. Admite rutas relativas y `~`. Al pulsar un directorio
  anterior se conservan los botones siguientes hasta abrir otra ruta.
- Clic derecho en el espacio vacío, sin seleccionar nada, permite abrir Konsole, crear una carpeta, añadir el directorio a marcadores,
  comprimirlo, copiar su ruta, actualizar y alternar archivos ocultos.
- El menú de carpetas e imágenes permite comprimirlas a ZIP en segundo plano. El archivo queda junto al original, sin sobrescribir archivos
  existentes; los enlaces simbólicos se conservan como enlaces.
- `Alt+←` y `Alt+→` recorren el historial; `Alt+↑` abre el directorio superior.
- `F5` actualiza el contenido y el espacio del disco. `Ctrl+H` o `Hidden` alterna los archivos ocultos.
- El doble clic o `Enter` abre las imágenes con el visualizador externo asociado mediante `xdg-open`; no hay visor integrado. `Esc` cancela
  la creación de carpetas, la edición de la dirección o la selección.

La barra de estado muestra el espacio disponible del sistema de archivos actual. Sin selección muestra espacio usado y total; al
seleccionar, muestra el tamaño del archivo o la suma recursiva de los archivos de la carpeta, incluidos los ocultos. Este cálculo se ejecuta
en segundo plano, no sigue enlaces simbólicos y señala resultados parciales cuando hay elementos inaccesibles. Los tamaños son lógicos, no
bloques ocupados.

Los marcadores, la vista, el zoom, el tema y la visibilidad de archivos ocultos se guardan en `$XDG_CONFIG_HOME/still-files/settings.json`
o, si la variable no está definida, en `~/.config/still-files/settings.json`.

Las miniaturas se guardan en WebP, con un máximo de 192 píxeles por lado, en `$XDG_CACHE_HOME/still-files/thumbnails` o
`~/.cache/still-files/thumbnails`. Se reutilizan entre ejecuciones y se regeneran si cambia el tamaño o la fecha de modificación del
original. La barra inferior muestra el progreso. Al salir de una carpeta se cancelan sus solicitudes pendientes; una decodificación en curso
puede terminar, pero su resultado se descarta sin guardarlo. Las miniaturas terminadas permanecen disponibles.

Las miniaturas admiten PNG, JPEG, GIF (primer fotograma), WebP, BMP, TIFF, ICO y EXR. Los EXR se convierten de RGB lineal a sRGB, sin mapeo
tonal, recortando al rango de pantalla; las miniaturas anteriores se regeneran. La memoria de decodificación se limita a 256 MiB. Para abrir
EXR, la aplicación externa asociada también debe admitirlos.

Los vídeos admiten MPG/MPEG, MP4, MOV (incluido ProRes), MKV, AVI, WebM, MXF y otros contenedores, según los decodificadores de FFmpeg
instalados. Se guarda un máximo de 8 fotogramas WebP de 192 píxeles por vídeo: el central al mostrarlo y los demás únicamente al explorar
sus posiciones con el cursor. La caché se reutiliza entre ejecuciones y se invalida al cambiar el original. Se usan dos trabajadores y un
máximo de 512 miniaturas en memoria, compartido con las imágenes. Las operaciones de vídeo se interrumpen al cambiar de carpeta o si superan
15 segundos; los formatos dañados o no compatibles muestran un icono y el error al pasar el cursor.

No se modifican los originales ni se implementan operaciones de copia, movimiento o borrado.

## Verificación

Desde `app/`:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo build --locked
```
