# Player

Visor en Rust para Linux, basado en la navegación de DJV 1.3.0 y las preferencias utilizadas durante el desarrollo. Arriba, zoom y
orientación; abajo, timeline con caché real y, debajo de él, una barra con reversa, parada, avance, bucle, FPS y fotograma actual. Sin
menús, audio, paneles ni herramientas adicionales.

## Ubicación en el repositorio

Este bloque es independiente de filemanager (`../app/`): conserva su manifiesto, dependencias, fuentes, documentación, licencia y lanzador.
Filemanager lo abre mediante `player/start.sh`, sin necesitar una copia externa. Los comandos siguientes se ejecutan desde `player/`.

## Ejecución

Requiere Rust 1.89 o posterior, OpenGL 3.3, FFmpeg y FFprobe en `PATH`. `Ctrl+O` utiliza Zenity; también se puede abrir por argumento o
arrastrando un archivo.

Ejecuta `./start.sh` para usar el binario actualizado sin pasar por Cargo. Al cerrar la ventana o pulsar `Ctrl+E`, Player permanece en
segundo plano y libera el medio abierto; la siguiente ejecución reutiliza el proceso y muestra la ventana. Si ya está visible, una nueva
apertura reutiliza esa misma ventana. `Ctrl+Q` o `./start.sh --quit` terminan el proceso por completo. No se inicia automáticamente con la
sesión ni guarda fotogramas en disco.

```console
cargo run --release -- /ruta/video.mov
cargo run --release -- /ruta/secuencia/imagen.1001.exr
cargo run --release -- '/ruta/secuencia/imagen.####.jpg'
cargo run --release -- '/ruta/secuencia/imagen.%04d.png'
```

Al seleccionar una imagen numerada, se agrupan los archivos con el mismo prefijo, extensión y relleno numérico compatible. Se admiten
secuencias dispersas y se conserva su numeración. El límite es de 50 000 imágenes, según la configuración original. Los patrones deben ir
entre comillas para evitar su expansión por el intérprete.

Los campos se confirman con Intro o al perder el foco. El zoom admite factores, como `0.75`, y porcentajes, como `75%`. FPS admite las
velocidades originales y fracciones exactas como `24000/1001`; la lista aparece al mantener el puntero sobre el campo.

Los botones superiores `−90°`, `0°` y `+90°` son exclusivos: giran a la izquierda, restauran el original o giran a la derecha, sin acumular
giros ni modificar el archivo. El ajuste al visor contempla el ancho y alto intercambiados; con zoom manual se conserva el centro de la
imagen.

## Formatos y ajustes

- Vídeos MP4, MOV y otros contenedores/códecs que pueda decodificar el FFmpeg instalado. Se utiliza el primer flujo de vídeo, sin audio ni
  rotación automática.
- Imágenes EXR, JPG, PNG, TIFF, BMP, TGA, HDR, PNM, WebP, DDS, ICO y QOI mediante lectores Rust; otros formatos, como DPX, recurren a
  FFmpeg. La compatibilidad depende del contenido y del decodificador, no solo de la extensión.
- Velocidad del vídeo conservada como fracción; imágenes a 24 FPS salvo metadatos `framesPerSecond` de EXR. Se incluyen los 16 valores
  originales, de 1 a 120 FPS.
- Inicio automático, repetición y reproducción de todos los fotogramas: si la decodificación no alcanza la velocidad, se ralentiza sin
  saltar fotogramas.
- Caché y precarga activadas. Límite de 24 GiB, sin reservarlos al iniciar; purga por orden de inserción, no LRU. La línea azul solo
  representa imágenes decodificadas residentes.
- Fondo negro, filtrado lineal, HUD inferior derecho inicialmente oculto y rueda de zoom del 10 %. EXR con gamma 2.2 y cuatro hilos de
  descompresión. Las imágenes de alta precisión se mantienen en coma flotante y se suben a una textura `RGBA32F`, sin convertirlas antes a
  ocho bits.
- Ajuste automático de ventana limitado al 75 % de la pantalla. `U` oculta los controles y restaura su visibilidad al salir. Paleta oscura y
  Noto Sans de 12 puntos cuando está instalada; fuente integrada como alternativa.

## Atajos

Los atajos se desactivan al editar texto para no interceptar la escritura.

| Acción                                                | Teclas                            |
| ----------------------------------------------------- | --------------------------------- |
| Parada / reversa                                      | `K` / `J`                         |
| Alternar reproducción y recuperar el sentido anterior | `Espacio`                         |
| Entrada / salida                                      | `Inicio` / `Fin`                  |
| Fotograma anterior / siguiente (pausa)                | `←` o `H` / `→` o `L`             |
| Anterior / siguiente                                  | `[` / `]`                         |
| Retroceder / avanzar 10                               | `Mayús+H` / `Mayús+L` o `{` / `}` |
| Retroceder / avanzar 100                              | `Ctrl+[` / `Ctrl+]`               |
| Cambiar bucle                                         | `;`                               |
| Girar hacia −90° / +90°, pasando por 0°               | `Y` / `I`                         |
| Marcar salida                                         | `O`                               |
| Restablecer entrada / salida                          | `Mayús+I` / `Mayús+O`             |
| Activar puntos de entrada/salida                      | `P`                               |
| Centrar / ajustar imagen / restablecer vista          | `Insert` / `Retroceso` / `Supr`   |
| Duplicar / reducir a la mitad / restablecer zoom      | `=` / `-` / `0`                   |
| Pantalla completa / ajustar imagen al visor           | `U` / `F`                         |
| Llenar el visor sin franjas (recorta sin deformar)    | `Ctrl+F`                          |
| Mostrar u ocultar interfaz / HUD                      | Acento grave / `Ctrl+H`           |
| Canal rojo / verde / azul / alfa / predeterminado     | `R` / `G` / `B` / `A` / `C`       |
| Guardar / mostrar fotograma de referencia             | `Mayús+E` / `E`                   |
| Abrir / cerrar archivo / salir                        | `Ctrl+O` / `Ctrl+W` / `Ctrl+Q`    |
| Cerrar ventana y permanecer en segundo plano          | `Ctrl+E`                          |
| Recargar archivo / fotograma                          | `Ctrl+R` / `Ctrl+Mayús+R`         |

`U` alterna la pantalla completa conservando el modo de ajuste elegido. `F` muestra toda la imagen y `Ctrl+F` llena el visor sin franjas,
conservando las proporciones y recortando lo que sobresale. El botón `Fit` de la barra superior alterna entre ambos ajustes y aparece
activado al rellenar el visor. `Ctrl+U` y `Ctrl+D` quedan libres.

El avance manual de fotogramas ignora las marcas de entrada/salida; la reproducción sigue respetándolas cuando están activadas.

En el visor: botón central, `Alt` o `Mayús` + arrastre izquierdo para mover; `Ctrl` + arrastre izquierdo para desplazar un fotograma por
cada cinco píxeles; `Ctrl` + clic central/derecho para duplicar/reducir el zoom. Rueda para zoom, `Mayús` + rueda para fotogramas y `Ctrl` +
rueda para velocidad. El zoom se ancla al puntero dentro del visor y al centro fuera de él.

En el timeline, cualquier botón permite buscar y arrastrar. El arrastre pausa temporalmente el reloj, permite salir del rango de
entrada/salida y conserva el estado de reproducción al soltar, como el widget original. No se han inventado atajos de flechas ni
interacciones de rueda para el timeline.

## Verificación y límites

```console
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo run -- --check-media /ruta/video.mp4
```

`--check-media` comprueba metadatos y decodifica el primer fotograma, el último y de nuevo el primero, sin abrir una ventana.

Esta implementación porta lógica observable; no incorpora el motor C++/Qt ni constituye todavía una certificación de equivalencia completa.
Véase [la auditoría](docs/AUDITORIA.md). En particular:

- Una búsqueda de vídeo fuera del flujo consecutivo decodifica desde el inicio hasta el índice exacto. Es correcta por índice, pero puede
  ser lenta en vídeos largos; la caché acelera la reproducción posterior y la reversa.
- Los vídeos se decodifican a RGBA de ocho bits, como la ruta FFmpeg original. No hay garantía de reproducción HDR de vídeo ni de gestión de
  color equivalente para todos los formatos. No se han portado los perfiles de película Cineon/DPX ni perfiles ICC/OCIO.
- EXR utiliza la primera capa RGB/RGBA válida. No hay selector de capas, soporte deep ni reproducción completa del HUD de metadatos. Las
  imágenes animadas abiertas como imágenes muestran su primer fotograma.
- No se garantiza un FPS mínimo ni un tamaño ilimitado: influyen la decodificación, la RAM y el límite de textura de la GPU. La caché puede
  llegar a consumir los 24 GiB configurados.
- Falta la comparación interactiva completa con DJV usando archivos del usuario y un gestor de ventanas real; las pruebas de pantalla
  virtual no certifican el cambio de modo del escritorio.

La aplicación no necesita DJV instalado ni lee su historial personal. La lógica de referencia conserva su licencia BSD en
[LICENSE](LICENSE); los avisos legales y las dependencias se detallan en la auditoría.
