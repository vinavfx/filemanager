# Instrucciones del proyecto

- Compila offline para Rocky 8 con la imagen local definida en `.github/release.Dockerfile` y garantiza permisos de ejecución (`chmod +x`)
  al compilar y copiar.
- Clippy y `cargo test` no sustituyen la compilación del ejecutable.
- Después de cada recompilación exitosa, cierra los procesos de la aplicación modificada, incluidos los que estén en segundo plano: Player
  si el cambio fue en `player/`, Filemanager si fue en `app/`, o ambos si se modificaron los dos. No cierres la aplicación no modificada.
  Esta instrucción autoriza el cierre de esos procesos.
- Indica si la compilación falla. Si tiene éxito, recuerda al usuario que debe volver a abrir la aplicación con `still-files` o `player`,
  según corresponda.
