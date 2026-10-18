# Auditoría y correspondencias

## Referencia utilizada

[DJV 1.3.0](https://github.com/grizzlypeak3d/DJV), commit `020d9fb885dda19307afb39015555e9552c90e43`. Durante el desarrollo se consultaron
sus fuentes, capturas y preferencias; este repositorio ya no incluye ese material. No se necesita una instalación de DJV en ejecución.

La confirmación del usuario establece Rust, apertura de EXR, MP4, MOV, JPG y otras secuencias o vídeos disponibles, sin menús, con barra
superior de zoom y orientación, y timeline inferior con una barra de reproducción debajo. Autoriza descargar dependencias y compilar. Se
mantienen excluidas las herramientas adicionales; los canales y el fotograma de referencia solo tienen atajos, sin paneles ni controles
nuevos.

## Módulos estudiados

| Fuente original                                                                               | Correspondencia en Player                                                                                                                                                                                                 |
| --------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `PlaybackGroup.cpp`, `PlaybackActions.cpp`, `PlaybackWidgets.cpp`, `PlaybackToolBar.cpp`      | Estados de transporte, sentido anterior, extremos, bucles, edición y pausa temporal del reloj.                                                                                                                            |
| `MiscWidget.cpp`, clase `FrameSlider`                                                         | Búsqueda con cualquier botón, arrastre sin detener el estado de transporte, numeración, marcas y caché. El widget no define navegación por rueda ni atajos propios de flechas.                                            |
| `ShortcutPrefs.cpp`, preferencias guardadas                                                   | Teclas del visor, transporte, puntos de entrada/salida, HUD y ventana.                                                                                                                                                    |
| `ViewGroup.cpp`, `ViewActions.cpp`, ambos `ImageView.cpp`, `MousePrefs.cpp`, `MainWindow.cpp` | Zoom por factores 2 y 0.5 desde teclado/clic, rueda por factor 1.1, anclaje al puntero, arrastre y desplazamiento por cinco píxeles. `Supr` restablece el origen inferior izquierdo; `Insert` centra sin cambiar el zoom. |
| `WindowGroup.cpp`, `WindowPrefs.cpp`, `MainWindow.cpp`                                        | Ajuste automático, máximo del 75 % y ocultación/restauración de controles al alternar pantalla completa.                                                                                                                  |
| `FileGroup.cpp`, `FileCache.cpp`, `FilePrefs.cpp`                                             | Precarga hacia delante con retorno al inicio; límite por bytes, fotogramas residentes y purga por antigüedad de inserción.                                                                                                |
| `FFmpegLoad.cpp`                                                                              | Velocidad del medio, índices desde cero, salida RGBA de ocho bits y diferencia entre lectura consecutiva y búsqueda.                                                                                                      |
| `OpenEXRLoad.cpp`, preferencias OpenEXR                                                       | Perfil gamma 2.2, datos de alta precisión y configuración de cuatro hilos.                                                                                                                                                |
| `Speed.cpp`, `Time.cpp`, `Sequence.cpp`, `Memory.cpp`                                         | Velocidades racionales, lista de 16 FPS originales, índices de secuencia y GiB binarios.                                                                                                                                  |

El código de Player está en `src/`; no es una envoltura del motor Qt.

## Decisiones técnicas

- `eframe`/`egui` 0.31.1 con OpenGL: interfaz sin menús y captura de teclado/ratón. Los campos tienen prioridad sobre los atajos durante la
  edición.
- Máquina de reproducción en Rust. La temporización acumula la fracción residual del intervalo y avanza como máximo un fotograma por
  presentación. La imagen actual debe estar decodificada antes de avanzar.
- Hilo de carga separado, generaciones por archivo y caché compartida. Los eventos antiguos se descartan, y abrir, cerrar o recargar
  invalida los fotogramas correspondientes. Los procesos de vídeo se terminan al reiniciar o liberar el decodificador.
- Lectores `image` y `exr` para imágenes; FFmpeg/FFprobe como procesos para vídeo y formatos adicionales. No se enlaza una versión
  particular de las bibliotecas C de FFmpeg. La reversa utiliza los mismos índices y la misma caché que el avance.
- Las búsquedas de vídeo utilizan `select=gte(n,índice)` desde el principio: evitan aproximaciones por FPS o por fotograma clave, pero son
  costosas en archivos largos. La lectura consecutiva reutiliza el proceso.
- Los datos EXR y de más de ocho bits se conservan como `f32`; el visor utiliza texturas `RGBA32F`. El código `unsafe` se limita a llamadas
  OpenGL, que requieren el contexto vigente y se ejecutan en el hilo gráfico. No hay accesos manuales a punteros de imágenes.
- Orientación manual mediante tres botones exclusivos: `−90°`, `0°` y `+90°`. El shader transforma las coordenadas de textura sin cambiar
  los píxeles en caché. El ajuste intercambia ancho y alto; con zoom manual se conserva el centro de la imagen.
- El alfa se interpreta como premultiplicado sobre negro. No se añade una desmultiplicación ni un perfil de visualización inexistente en la
  configuración guardada.
- La caché usa orden de inserción. No replica la colisión accidental entre marcas de tiempo del mismo segundo presente en la estructura
  original de purga.

## Validación y trabajo pendiente

Se han ejecutado pruebas temporales de velocidades racionales, tres bucles, reversa, recuperación del sentido anterior, puntos de
entrada/salida y reloj sin saltos. Los medios sintéticos cubren MP4/H.264, MOV/ProRes, JPG numerado, patrones, EXR y DPX, con lectura del
principio, del final y vuelta al principio. Las comprobaciones de interfaz se realizan en Xvfb con representación OpenGL por software.

No existen pruebas permanentes añadidas por iniciativa del agente. `cargo test` comprueba la compilación del objetivo de pruebas, pero no
representa por sí solo una comparación funcional con DJV.

Quedan pendientes la comparación visual y temporal completa con la instalación de referencia, una matriz amplia de formatos/códecs,
reproducción bajo presión de memoria y la certificación de pantalla completa con un gestor de ventanas real. No se declara equivalencia
integral ni rendimiento garantizado. Las diferencias de color, capas EXR, animaciones y búsqueda de vídeo están descritas en el README.

## Licencias

La lógica portada se basa en DJV bajo BSD de tres cláusulas; el aviso original completo se conserva en `LICENSE`. No se distribuyen sus
bibliotecas Qt ni sus binarios.

Las bibliotecas Rust se resuelven mediante `Cargo.lock`; sus licencias permanecen en sus paquetes de Cargo: `eframe`/`egui`, `image`, `exr`,
`rayon-core`, `bytemuck` y `serde_json`, además de las dependencias transitivas. FFmpeg y Zenity son ejecutables externos, no copiados al
repositorio. Antes de distribuir un binario empaquetado deben recopilarse los avisos de las versiones realmente incluidas y revisar por
separado la licencia de la distribución de FFmpeg utilizada.
