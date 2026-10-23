# Backends gráficos con sincronización X11

Copias locales de **winit 0.30.13** y **eframe 0.31.1**, procedentes de sus paquetes publicados en crates.io. Se conservan los encabezados,
metadatos de autoría y licencias originales; ambas copias se distribuyen bajo la licencia Apache 2.0 incluida en cada directorio.

Se incluyen fuentes y recursos de compilación. Se omiten ejemplos, pruebas externas e historiales de versiones de winit; su módulo de
historial no forma parte de esta copia. No se modifican las versiones ni la caché global de Cargo.

## Cambios funcionales

- `winit`: activación explícita de `_NET_WM_SYNC_REQUEST`, contador por ventana, asociación de solicitud y `ConfigureNotify`, consulta del
  número pendiente y confirmación posterior a la presentación. El contador se libera al destruir la ventana.
- `eframe/glow`: activa el contrato por ventana y confirma solo después de un intercambio correcto y `glFinish`, tanto en la raíz como en
  los viewports inmediatos. No espera a la GPU en cuadros sin una solicitud pendiente. Wayland no cambia.
- `app/Cargo.toml` sustituye ambas dependencias de crates.io por estas copias; `Cargo.lock` conserva sus versiones.

Para integrar otra aplicación egui/glow, reutilizar las dos copias y las sustituciones del manifiesto. Copiar solo una no completa el
contrato. No confirmar desde `App::update`, desde Damage ni antes de intercambiar el búfer.

El contrato completo y la verificación están en [Sincronización X11 de Still Files](../../docs/sincronizacion-x11.md).

## Mantenimiento

Al actualizar las dependencias, comparar estas modificaciones con la nueva versión y conservar las pruebas de cuadros incompletos,
confirmaciones obsoletas, respuesta tardía y ventanas secundarias. Solo anunciar el protocolo desde un renderizador que pueda cumplir la
confirmación.

Compilar Still Files offline para Rocky 8 con la imagen local `still-files-release:latest`, montando el repositorio en `/work` y la caché de
Cargo en `/opt/cargo/registry`; ejecutar `cargo build --offline --locked --release` desde `/work/app`. La imagen usa Rust 1.89. No requiere
compilar Player.
