# Still Files

Gestor de archivos minimalista para Linux, escrito en Rust con egui. Todo está en este repositorio: la raíz contiene únicamente el lanzador
`start.sh` y `app/` contiene el proyecto.

## Arranque

Requiere Rust 1.89 o superior, un escritorio X11 o Wayland y OpenGL. Para abrir otros archivos utiliza `xdg-open`; la opción de terminal
requiere Konsole. La búsqueda utiliza `fzf`, disponible en `PATH` o `~/.fzf/bin/fzf`, sin mostrar su interfaz. La compresión utiliza `zip`.
Las previsualizaciones de vídeo requieren `ffmpeg` y `ffprobe`.

Desde la raíz:

```bash
./start.sh
./start.sh /ruta/a/una/carpeta
```

El lanzador descarga las dependencias y compila si es necesario. Las siguientes ejecuciones reutilizan la compilación. La interfaz está en
inglés. Los iconos de `Hidden`, `Sequences` y `Recursive media` muestran su nombre al pasar el cursor.

## Uso

- Un clic selecciona; doble clic o `Enter` abre. `Esc` quita la selección sin olvidar el último icono: el siguiente movimiento con flechas o
  `h/j/k/l` vuelve a seleccionarlo si sigue en la vista actual. Si hay un aviso, `Esc` lo cierra primero sin cambiar la selección.
- `Ctrl+A` selecciona todos los elementos de la vista actual; en `Tree` incluye las carpetas desplegadas, no el contenido de las cerradas.
  En los campos de texto conserva la selección de texto habitual.
- `Ctrl+V` activa Visual Line desde el icono actual. `h/j/k/l` o las flechas amplían o reducen el rango seleccionado, también en `Tree`;
  `gg`, `G`, `Ctrl+D` y `Ctrl+U` permiten seleccionar hasta otros puntos. `d` envía los seleccionados a la papelera sin confirmación. `Esc`
  o `Ctrl+V` salen del modo y dejan seleccionado solo el primer icono del rango, de arriba hacia abajo. La barra de estado indica
  `VISUAL LINE`. Fuera de Visual Line, `dd` envía la selección a la papelera sin confirmación; una sola `d` no hace nada. En ambos modos,
  `u` permite deshacer el borrado. Si no se puede usar la papelera, se muestra un error sin borrar permanentemente. Vaciar la papelera sí
  pide confirmación porque es irreversible. `gg` y `Mayús+G` solo mueven el cursor al principio o al final, sin ampliar la selección.
- `yy` copia la selección a un registro interno. En Visual Line basta `y`: copia, sale del modo y deja seleccionado el primer icono del
  rango. Una sola `y` fuera de Visual no copia; otra tecla, un clic o perder el foco cancela la secuencia pendiente. `p` pega después del
  icono seleccionado; en `Tree`, una carpeta cerrada recibe las copias debajo, como elementos hermanos, y una abierta las recibe dentro,
  antes de sus hijos. Con un hijo seleccionado, pega después de ese hijo. Sin selección, pega al principio de la carpeta actual. Copia
  archivos, carpetas, enlaces simbólicos y todos los fotogramas de las secuencias, sin sobrescribir: añade ` (copy)` y un número si el
  nombre ya existe. Al terminar, quedan seleccionadas todas las copias pegadas. `u` deshace y `Ctrl+R` rehace: conservan la selección y, si
  desaparece, eligen el icono superior más cercano que siga existiendo; si no hay ninguno arriba, eligen el siguiente. El orden de pegado se
  conserva al reiniciar; pulsar un encabezado de `Tree` restaura la ordenación de esa carpeta y sus subcarpetas. Las copias reutilizan las
  miniaturas del original en memoria y disco, sin decodificarlas de nuevo; al modificar un archivo, su miniatura se actualiza
  independientemente. Estos atajos no interfieren con los campos de texto.
- `n` edita el nombre del elemento seleccionado y `Enter` confirma. Renombrar vincula la caché existente al nuevo nombre y conserva la
  textura en memoria, también al deshacer y rehacer, sin volver a decodificar ni bloquear la interfaz esperando miniaturas.
- En `Tree`, `Alt+P` selecciona el padre visible del elemento actual sin plegar carpetas ni cambiar de directorio.
- En `Tree`, `o` abre o cierra la carpeta seleccionada. `Mayús+O` alterna entre expandir recursivamente sus subcarpetas y cerrarlas todas,
  manteniendo abierta la carpeta seleccionada. No sigue enlaces simbólicos y respeta los archivos ocultos configurados. Cada expansión
  recursiva se limita a 5 niveles, 64 carpetas y 2000 entradas examinadas, incluidos los recuentos de contenido de las carpetas, con un
  presupuesto de 250 ms comprobado entre lecturas. Al alcanzar un límite, muestra un aviso; `o` permite abrir manualmente lo restante.
- Los archivos sin miniatura usan los iconos MIME del tema activo del sistema, consultados mediante `gio`. Los iconos SVG/XPM se rasterizan
  con `gdk-pixbuf-thumbnailer`. La carga se realiza en segundo plano y se reutiliza por extensión; si faltan herramientas o un icono, se
  conserva el genérico. El tema se detecta al iniciar la aplicación.
- Las carpetas con contenido muestran dos hojas dentro del icono; las vacías mantienen su aspecto anterior. El indicador utiliza el recuento
  existente, incluidos los archivos ocultos, y se actualiza con `F5`.
- `Light`/`Dark`, en la barra superior, alterna entre los temas claro y oscuro; la elección se conserva al reiniciar.
- Añade marcadores con el botón `+`, desde el menú contextual o arrastrando una carpeta desde `Grid` o `Tree` a la barra de marcadores.
- Los accesos a la carpeta personal y sus carpetas principales aparecen arriba, con iconos propios y rutas XDG. Clic derecho permite
  quitarlos sin borrar carpetas; el menú `...` restaura los accesos predeterminados.
- `Tree` muestra carpetas expandibles, filas con fondos alternos y las columnas `Name`, `Size` y `Date Modified`. En carpetas, `Size` indica
  el número de elementos inmediatos, incluidos los ocultos; en archivos, el tamaño en bytes. Las fechas se muestran en la zona horaria local
  mediante `date`; en secuencias se usa la modificación más reciente. Pulsar un encabezado ordena cada nivel del árbol por esa columna;
  pulsarlo de nuevo invierte el orden. Una flecha indica el sentido y las carpetas permanecen primero. La elección se conserva al reiniciar.
  `F5` actualiza estos datos. `Grid` muestra todos los archivos con iconos. `Media` muestra solo imágenes y vídeos en un mosaico con margen
  entre miniaturas; solo las carpetas muestran el nombre, centrado. Los archivos no muestran títulos ni rutas al pasar el cursor o
  seleccionar. Las tres vistas cargan miniaturas en segundo plano. Los vídeos muestran una imagen del centro; mueve el cursor de izquierda a
  derecha para recorrer muestras desde el inicio hasta el final, sin reproducción continua. El zoom cambia el tamaño de las miniaturas y los
  iconos, no el de la letra.
- `Alt+R` alterna `Recursive media` sin cambiar de vista. Este control, junto a `Sequences` y `Hidden`, muestra únicamente imágenes, vídeos
  y secuencias de la carpeta actual y todas sus subcarpetas en `Tree`, `Grid` y `Media`, sin carpetas ni otros archivos. El recorrido se
  ejecuta en segundo plano, respeta `Hidden` y `Sequences`, agrupa secuencias por carpeta y no sigue enlaces simbólicos a carpetas. Indica
  si omite directorios inaccesibles. Al desactivarlo se recupera el contenido habitual. Cada ventana nueva inicia con esta opción
  desactivada.
- En `Media`, las miniaturas no tienen margen y la selección se muestra con una capa de color semitransparente, sin contorno.
- `Alt+S` o `Sequences`/`Images`, en la barra superior, alterna entre agrupar imágenes numeradas con el mismo nombre base y extensión o
  mostrarlas individualmente. Los saltos de numeración no separan los grupos. Cada grupo muestra los intervalos existentes separados por
  espacios, con números sueltos para fotogramas aislados, y el total de archivos; admite explorar hasta ocho muestras con el cursor y guarda
  únicamente esas miniaturas. El doble clic abre el primer fotograma. La barra dentro del borde inferior de las miniaturas de vídeos y
  secuencias, de seis píxeles de grosor y con fondo contrastado, indica la posición de la muestra mostrada, incluso mientras se carga otra.
- `Ctrl+F` busca por nombre entre los elementos visibles y selecciona la mejor coincidencia mientras escribes, sin filtrar ni ocultar nada.
  Ignora mayúsculas y prioriza nombres exactos, prefijos y fragmentos contiguos; también admite letras en orden con caracteres intermedios.
  En `Tree` incluye los elementos visibles de las carpetas expandidas, nunca el contenido de las cerradas. `Enter` o `Esc` cierra el campo
  conservando la selección, sin abrir el archivo.
- La lupa de la barra superior o `Ctrl+P` convierte el campo de direcciones en un buscador recursivo de nombres y rutas. Utiliza el motor de
  fzf con su coincidencia difusa, ordenación y sintaxis habituales: `'texto` busca literalmente, `^texto` por prefijo, `texto$` por sufijo,
  `!texto` excluye y `|` separa alternativas. Los términos separados por espacios se combinan. No se cargan `FZF_DEFAULT_OPTS` ni
  `FZF_DEFAULT_OPTS_FILE`, para evitar opciones de terminal incompatibles con la integración. El recorrido y la búsqueda se ejecutan en
  segundo plano; los resultados aparecen progresivamente y las consultas anteriores se cancelan. Se muestran los mejores 1000 resultados y
  el número total de coincidencias. `Location` muestra la ruta absoluta de la carpeta que contiene cada resultado. `↑`/`↓` seleccionan; con
  el buscador activo, `Ctrl+H`/`Ctrl+K`/`Ctrl+P` seleccionan el resultado anterior y `Ctrl+J`/`Ctrl+L`/`Ctrl+N` el siguiente, sin modificar
  la consulta. Las letras sin `Ctrl` sirven para escribir. En este contexto, `Ctrl+H` no alterna los ocultos y `Ctrl+L` no edita la
  dirección. `Enter` sale de la búsqueda y selecciona el resultado centrado en su carpeta, desactivando `Recursive media`; si pertenece a
  una secuencia, selecciona el grupo. En `Media`, los archivos no multimedia se revelan en `Grid`. El doble clic abre; `Esc` o la lupa
  restaura la dirección. Se respeta `Hidden`, no se recorren enlaces simbólicos y se indican los elementos inaccesibles. La búsqueda muestra
  archivos individuales, aunque esté activado `Sequences`. `F5` vuelve a recorrer las carpetas. Se reutiliza el índice mientras la búsqueda
  está abierta; el tiempo total incluye el recorrido del disco, el proceso de fzf y la interfaz, por lo que no se garantiza la misma
  latencia que fzf sobre una lista ya cargada.
- `Ctrl+L` o `Edit` alterna entre la dirección editable y los botones de ruta. Admite rutas relativas y `~`. Mientras escribes, muestra en
  gris la continuación de una carpeta coincidente. `Tab` completa y recorre las coincidencias; `Mayús+Tab` retrocede. Solo sugiere carpetas,
  incluidos enlaces a carpetas; los nombres que empiezan por punto requieren escribir el punto o activar `Hidden`. `Enter` abre la ruta. Al
  pulsar un directorio anterior se conservan los botones siguientes hasta abrir otra ruta.
- Clic derecho en el espacio vacío, sin seleccionar nada, permite abrir Konsole, crear una carpeta, añadir el directorio a marcadores,
  comprimirlo, copiar su ruta, actualizar y alternar archivos ocultos.
- El menú de carpetas e imágenes permite comprimirlas a ZIP en segundo plano. El archivo queda junto al original, sin sobrescribir archivos
  existentes; los enlaces simbólicos se conservan como enlaces.
- `Ctrl+O`/`Alt+←` y `Ctrl+I`/`Alt+→` recorren el historial. `gT` o `Alt+↑` sube al directorio padre; `gt` baja por la ruta conservada.
  Funcionan desde el panel de archivos y el lateral, sin interferir con la edición de texto.
- `Alt+T` activa `Tree`, `Alt+G` activa `Grid` y `Alt+M` activa `Media` junto con `Recursive media`, incluso si ya estás en `Media`. Las
  tres vistas siguen disponibles en la barra superior.
- `F5` actualiza el contenido y el espacio del disco. `Ctrl+H` o `Hidden` alterna los archivos ocultos en ambos paneles.
- El doble clic o `Enter` abre las imágenes con el visualizador externo asociado mediante `xdg-open`; no hay visor integrado. `Esc` cancela
  la creación de carpetas, la edición de la dirección o la selección.

La barra de estado muestra el espacio disponible del sistema de archivos actual. Sin selección muestra espacio usado y total; al
seleccionar, muestra el tamaño del archivo o la suma recursiva de los archivos de la carpeta, incluidos los ocultos. Este cálculo se ejecuta
en segundo plano, no sigue enlaces simbólicos y señala resultados parciales cuando hay elementos inaccesibles. Los tamaños son lógicos, no
bloques ocupados.

Los marcadores se comparten con Caja mediante `$XDG_CONFIG_HOME/gtk-3.0/bookmarks` o `~/.config/gtk-3.0/bookmarks`: una URI por línea y un
nombre opcional separado por un espacio. Se conservan el orden y los nombres; las altas, bajas y modificaciones de Caja se detectan cada
segundo, y los cambios de este gestor se guardan en el mismo archivo. Si no existe, se lee `~/.gtk-bookmarks` como alternativa heredada. Los
marcadores antiguos de `settings.json` se ignoran. Como Caja, se ocultan las rutas locales inexistentes y los duplicados de las carpetas
predeterminadas, sin eliminarlos del archivo; su disponibilidad se comprueba cada segundo. Las ubicaciones remotas se abren mediante
`xdg-open`.

La vista, el zoom, el margen de Media, el tema, la agrupación de secuencias, los accesos predeterminados ocultos y la visibilidad de
archivos ocultos se guardan en `$XDG_CONFIG_HOME/still-files/settings.json` o `~/.config/still-files/settings.json`.

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
15 segundos; los formatos dañados o no compatibles muestran un icono genérico.

No se modifican los originales ni se implementan operaciones de copia, movimiento o borrado.

## Verificación

Desde `app/`:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo build --locked
```
