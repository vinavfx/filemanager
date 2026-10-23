# Sincronización X11 de Still Files

Still Files implementa el protocolo estándar
[`_NET_WM_SYNC_REQUEST`](https://specifications.freedesktop.org/wm-spec/latest/ar01s06.html#id-1.7.3) con sus backends locales de winit y
eframe/glow. No utiliza una pausa fija ni interpreta Damage como confirmación de cuadro completo.

## Orden obligatorio

1. El renderizador activa el protocolo por ventana nativa: crea un contador XSync, publica `_NET_WM_SYNC_REQUEST_COUNTER` como `CARDINAL/32`
   y añade `_NET_WM_SYNC_REQUEST` a `WM_PROTOCOLS` sin eliminar los demás protocolos.
2. El gestor envía un `ClientMessage` de tipo `WM_PROTOCOLS`, formato 32: protocolo en `data[0]`, fecha en `data[1]` y número de solicitud
   de 64 bits en `data[2]` —parte baja— y `data[3]` —parte alta—. Después envía la configuración correspondiente.
3. winit conserva el número recibido y lo asocia con el siguiente `ConfigureNotify`, incluido su tamaño. Si se acumulan solicitudes, utiliza
   el último destino procesado. La solicitud por sí sola no confirma ningún cuadro.
4. eframe compone ese tamaño, dibuja y realiza el intercambio de búfer. Solo si tiene éxito y la solicitud sigue vigente, completa el
   trabajo de GPU con `glFinish` y actualiza el contador al número solicitado.
5. El gestor incorpora contenido, decoración y geometría cuando recibe la confirmación. Mientras tanto conserva una copia independiente del
   cuadro anterior de esa ventana, sin bloquear las otras.
6. Al destruir la ventana se destruye el contador. El gestor puede reinicializarlo al gestionar la ventana: no debe suponerse un valor
   inicial.

La ruta se aplica a la raíz y a los viewports inmediatos, incluidas las ventanas secundarias y el escritorio de Still Files. `glFinish` solo
se solicita cuando existe una confirmación pendiente; el dibujo normal no incorpora esa espera. Wayland conserva su funcionamiento propio.

Si falla el intercambio o la conexión X11, no se confirma un cuadro que no se ha presentado. Vina conserva la imagen anterior incluso
después del aviso de diagnóstico de 150 ms y acepta una respuesta tardía; no convierte el plazo en una confirmación ficticia.

## Integrar otra aplicación

Reutilizar **ambos** backends de `app/vendor/` y el bloque `[patch.crates-io]` de `app/Cargo.toml`. El soporte de winit se activa desde el
renderizador, no automáticamente para cualquier aplicación que utilice la biblioteca.

No incrementar el contador desde `App::update`, un callback de pintura anterior al intercambio o un evento Damage. Otros motores de dibujo
deben confirmar después de su propia finalización de presentación. Cada ventana nativa necesita su contador y estado independientes.

La implementación está en:

- `app/vendor/winit/src/platform_impl/linux/x11/{window,event_processor}.rs`: contador, recepción y asociación con la configuración.
- `app/vendor/winit/src/platform/x11.rs`: API `WindowExtX11` para activar, consultar y completar solicitudes.
- `app/vendor/eframe/src/native/glow_integration.rs`: confirmación posterior al intercambio en ambas rutas de renderizado.

Los metadatos de procedencia y mantenimiento están en [Backends gráficos](../app/vendor/README.md). El contrato del compositor se documenta
además en `vina_desktop/docs/sincronizacion-x11.md`, en el repositorio de Vina Desktop.

## Compilar y verificar

Compilar offline con la imagen local Rocky 8 `still-files-release:latest`, Rust 1.89, el repositorio montado en `/work` y la caché del
anfitrión en `/opt/cargo/registry`. Desde `/work/app`, ejecutar `cargo build --offline --locked --release`. No requiere recompilar Player.

Ejecutar también `cargo fmt --all -- --check`, `cargo clippy --offline --all-targets --all-features -- -D warnings` y `cargo test --offline`
desde `app/`. Las copias locales no requieren descargas ni modificaciones de la caché global.

En una pantalla X11 aislada y con configuración propia:

- Verificar las propiedades y confirmaciones de la ventana principal y las secundarias.
- Dibujar un cuadro incompleto deliberadamente, retrasar la confirmación y comprobar que no aparece en la salida.
- Comprobar confirmaciones obsoletas, espera superior al plazo de diagnóstico, actividad de otra ventana y recuperación posterior.
- Comparar los píxeles del cliente terminado y de la salida tras redimensionados repetidos; comprobar movimientos sin nuevas solicitudes.
- Probar maximización, pantalla completa, minimización, cierre, sustitución del gestor y liberación de recursos.
- Medir solicitud → confirmación y solicitud → presentación por separado.

Se verificaron dos ventanas reales, veinte redimensionados con comparación de píxeles, diez movimientos sin solicitudes adicionales,
maximización, pantalla completa, minimización y restauración. El cierre de la ventana secundaria destruyó su contador. La sonda del gestor
rechazó dibujos incompletos y confirmaciones obsoletas, mantuvo otra ventana activa y aceptó una respuesta retrasada 300 ms.

Xvfb verifica protocolo y píxeles; no certifica sincronización física ni ausencia de tearing en otros servidores, controladores o monitores.
