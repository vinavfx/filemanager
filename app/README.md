# Still Files

Gestor de archivos minimalista para Linux, escrito en Rust con egui. Todo está en este repositorio: la raíz contiene únicamente el lanzador
`start.sh` y `app/` contiene el proyecto.

## Arranque

Requiere Rust 1.89 o superior, un escritorio X11 o Wayland y OpenGL. Para abrir otros archivos utiliza `xdg-open`; la opción de terminal
requiere Konsole.

Desde la raíz:

```bash
./start.sh
./start.sh /ruta/a/una/carpeta
```

El lanzador descarga las dependencias y compila si es necesario. Las siguientes ejecuciones reutilizan la compilación. La interfaz está en
inglés.

## Uso

- Un clic selecciona; doble clic o `Enter` abre.
- Selecciona una carpeta y pulsa `Ctrl+D` para crear un marcador. Clic derecho sobre el marcador permite quitarlo.
- `Tree` muestra carpetas expandibles; ambas vistas, `Tree` y `Grid`, muestran miniaturas cargadas en segundo plano para las imágenes
  visibles. El deslizador inferior cambia su tamaño.
- `Ctrl+L` o `Edit` activa la dirección editable; `Path` vuelve a los botones de ruta. Admite rutas relativas y `~`. Al pulsar un directorio
  anterior se conservan los botones siguientes hasta abrir otra ruta.
- Clic derecho en el espacio vacío permite abrir Konsole en el directorio actual, añadirlo a marcadores, copiar la ruta, actualizar y
  alternar archivos ocultos. Las carpetas también permiten abrir Konsole desde su menú.
- `Alt+←` y `Alt+→` recorren el historial; `Alt+↑` abre el directorio superior.
- `F5` actualiza el contenido y el espacio del disco. `Ctrl+H` o `Hidden` alterna los archivos ocultos.
- Las imágenes se abren en un visor con zoom, ajuste y botones anterior/siguiente. `Esc` cierra el visor, cancela la edición o limpia la
  selección.

La barra de estado muestra el espacio disponible del sistema de archivos actual. Sin selección muestra espacio usado y total; al
seleccionar, muestra el tamaño del archivo o la suma recursiva de los archivos de la carpeta, incluidos los ocultos. Este cálculo se ejecuta
en segundo plano, no sigue enlaces simbólicos y señala resultados parciales cuando hay elementos inaccesibles. Los tamaños son lógicos, no
bloques ocupados.

Los marcadores, la vista, el zoom y la visibilidad de archivos ocultos se guardan en `$XDG_CONFIG_HOME/still-files/settings.json` o, si la
variable no está definida, en `~/.config/still-files/settings.json`.

Las miniaturas se guardan en WebP, con un máximo de 192 píxeles por lado, en `$XDG_CACHE_HOME/still-files/thumbnails` o
`~/.cache/still-files/thumbnails`. Se reutilizan entre ejecuciones y se regeneran si cambia el tamaño o la fecha de modificación del
original. La barra inferior muestra el progreso. Al salir de una carpeta se cancelan sus solicitudes pendientes; una decodificación en curso
puede terminar, pero su resultado se descarta sin guardarlo. Las miniaturas terminadas permanecen disponibles.

El visor muestra inmediatamente la miniatura disponible mientras carga la imagen y conserva las cuatro últimas vistas en memoria. Admite
PNG, JPEG, GIF (primer fotograma), WebP, BMP, TIFF, ICO y EXR, con conversión HDR para pantalla. Limita la memoria de decodificación a 256
MiB y la vista a 4096 píxeles por lado. No modifica originales; no implementa operaciones de copia, movimiento ni borrado.

## Verificación

Desde `app/`:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo build --locked
```
