# Instrucciones del proyecto

- Después de modificar el comportamiento de la aplicación, compila inmediatamente el ejecutable con `cd app && cargo build --offline` para
  que el usuario pueda probarlo. Esta instrucción autoriza esa compilación, pero no la descarga de dependencias.
- Clippy y `cargo test` no sustituyen la compilación del ejecutable.
- Indica si la compilación falla. Si tiene éxito, recuerda al usuario que debe cerrar la aplicación y abrirla con `./start.sh`.
