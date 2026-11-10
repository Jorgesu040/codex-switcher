# Prueba de reasignación de proyectos locales

Fork experimental de Lampese/codex-switcher, basado en `45ada5e` (rama principal).
La última versión publicada al revisarlo era v0.2.20; la instalación examinada
también era 0.2.20. Ni esa versión ni la rama principal trataban
`electron-account-renderer-state-v1`.

Existe una propuesta independiente para resolverlo, todavía abierta al revisar:
https://github.com/Lampese/codex-switcher/pull/233
Este fork no incorpora esa PR; implementa una prueba optativa de reasignación,
sin propagar eliminaciones entre cuentas.

La aplicación de Codex examinada, 26.1103.7773.0, identifica el propietario de
la configuración mediante SHA256 del JSON compacto `[userId, accountId]`.
Conserva las configuraciones de las otras cuentas en `inactive`.

Esta prueba asocia la configuración existente de los proyectos locales al
identificador de la cuenta elegida. Conserva los IDs de los proyectos y sus
relaciones con los chats. No modifica `auth.json`, los JWT, las transcripciones
ni las bases SQLite. No elimina el registro de propietarios ni el estado
original de la otra cuenta. Los proyectos de ChatGPT y los proyectos remotos
no se transfieren. La visibilidad en la interfaz debe verificarse después de
reiniciar Codex; las pruebas automáticas verifican el archivo resultante.

## Primera prueba en Windows

1. Cierra Codex completamente y termina cualquier operación local en curso.
2. Cierra la versión original de Codex Switcher desde su bandeja.
3. Extrae el paquete portátil y ejecuta `Test-Local-Projects.cmd`.
4. El lanzador muestra el identificador origen, el destino y el número de
   proyectos locales, y aplica la prueba si Codex está cerrado.
5. Abre Codex con la misma cuenta y comprueba la barra lateral.

También puedes usar PowerShell desde la carpeta del paquete:

```powershell
.\codex-local-projects.exe                  # solo vista previa
.\codex-local-projects.exe --apply          # Codex debe estar cerrado
```

La herramienta rechaza la escritura si detecta Codex abierto. No cierra
procesos automáticamente. Respeta `CODEX_HOME` cuando está definido.

## Uso del switcher modificado

Ejecuta `Start-Local-Projects.cmd`. Este lanzador activa
`CODEX_SWITCHER_REBIND_LOCAL_PROJECTS=1` únicamente para el proceso del fork.
El resto del cambio de cuenta sigue el flujo normal del programa: Codex debe
estar cerrado. Antes de escribir las credenciales de destino, el fork prepara
su configuración de proyectos locales. Abrir el EXE directamente deja la prueba
automática desactivada. El fork conserva el almacén de cuentas original del
switcher, sin importar ni exportar credenciales.

Si hay proyectos en la cuenta actual, se utiliza esa configuración como origen.
Si no los hay y existe exactamente un perfil inactivo con proyectos locales,
se utiliza ese perfil. Si hay varios posibles orígenes, se detiene en vez de
elegir uno arbitrariamente. La herramienta independiente permite elegir:

```powershell
.\codex-local-projects.exe --source-owner HASH --apply
```

Los valores existentes de la cuenta destino tienen prioridad. Las asociaciones
de proyectos y los proyectos que ya tenía se conservan. Se reactiva la migración
local idempotente de Codex para que procese las asociaciones recuperadas.

## Deshacer

Cada modificación guarda los bytes originales de `.codex-global-state.json`
y de su `.bak`, si existía, en:

`CODEX_HOME/project-rebind-backups/<identificador-único>/`

La ruta exacta se imprime al aplicar la prueba. Con Codex cerrado:

```powershell
.\codex-local-projects.exe --restore "C:\Users\TU_USUARIO\.codex\project-rebind-backups\IDENTIFICADOR"
```

La restauración conserva además una copia del estado que reemplaza. Estas copias
son configuración local, pueden contener rutas e historial del editor y deben
permanecer en tu equipo. No se incluyen en el repositorio ni en el paquete.

## Compilar

Requiere Node, pnpm, Rust y las herramientas C++ de Visual Studio en Windows.

```powershell
pnpm install --frozen-lockfile
pnpm build
cargo test --manifest-path src-tauri/Cargo.toml --lib auth::local_projects
pnpm tauri:win build --no-bundle --config src-tauri/tauri.local-projects.conf.json
cargo build --manifest-path src-tauri/Cargo.toml --release --bin codex-local-projects
```

El archivo de configuración del fork cambia el nombre y el identificador de la
aplicación, desactiva los artefactos de actualización firmados y deja vacía la
lista de servidores de actualización. No reemplaza la instalación original.
