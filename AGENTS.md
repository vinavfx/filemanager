# Instrucciones del proyecto

- Después de modificar el comportamiento de la aplicación, compila inmediatamente el ejecutable con `cd app && cargo build --offline` para
  que el usuario pueda probarlo. Esta instrucción autoriza esa compilación, pero no la descarga de dependencias.
- Clippy y `cargo test` no sustituyen la compilación del ejecutable.
- Después de cada recompilación exitosa, cierra los procesos de la aplicación modificada, incluidos los que estén en segundo plano: Player
  si el cambio fue en `player/`, Filemanager si fue en `app/`, o ambos si se modificaron los dos. No cierres la aplicación no modificada.
  Esta instrucción autoriza el cierre de esos procesos.
- Indica si la compilación falla. Si tiene éxito, recuerda al usuario que debe volver a abrir la aplicación con `./start.sh` o
  `./player/start.sh`, según corresponda.
