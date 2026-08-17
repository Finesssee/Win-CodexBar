# Win-CodexBar

[English](./README.md) | [简体中文](./README.zh-CN.md) | [繁體中文（臺灣）](./README.zh-TW.md) | [日本語](./README.ja-JP.md) | [한국어](./README.ko-KR.md) | [Español mexicano](./README.es-MX.md) | [Türkçe](./README.tr-TR.md)

Win-CodexBar es una aplicación de bandeja del sistema para Windows que mantiene visible el uso de herramientas de codificación con IA sin necesidad de abrir una docena de paneles. Traslada el espíritu de [CodexBar](https://github.com/steipete/CodexBar) a un entorno de escritorio Tauri + React respaldado por lógica compartida de proveedores en Rust.

<table>
  <tr>
    <td width="36%" align="center">
      <img src="docs/images/tray-panel.png" alt="Panel de bandeja de Win-CodexBar mostrando tarjetas de uso de proveedores"/>
    </td>
    <td width="64%" align="center">
      <img src="docs/images/settings-providers.png" alt="Página de configuración de Proveedores de Win-CodexBar"/>
    </td>
  </tr>
</table>

## Destacados

- **48 proveedores** incluyendo Codex, Claude, Copilot, OpenRouter, Cursor, Gemini, DeepSeek, MiniMax, Kiro, Antigravity, Groq y más.
- **Flujo centrado en la bandeja** con una cuadrícula compacta de proveedores, tarjetas de uso, acción de actualización, acceso directo a configuración y control de cierre.
- **Configuración por proveedor** para selección de fuente, credenciales, importación de cookies, cuentas de token, claves API, regiones y preferencias de visualización en bandeja.
- **Protección de credenciales en Windows** para claves API gestionadas por la aplicación, cookies manuales y cuentas de token, utilizando DPAPI con ámbito de usuario cuando está disponible.
- **Importación de cookies del navegador** para Chrome, Edge, Brave y Firefox, con activación opcional por proveedor.
- **CLI local instalada** para consultar uso, costo, configuración, diagnóstico e integraciones de bucle local.
- **Compilaciones con instalador y portable** con arranque de WebView2 Runtime, arranque de VC++ Runtime y archivos de suma de verificación SHA-256.

## Instalación

Instalar con el Administrador de Paquetes de Windows:

```powershell
winget install Finesssee.Win-CodexBar
```

O descarga la última versión (instalador o portable) desde [GitHub Releases](https://github.com/Finesssee/Win-CodexBar/releases).

- Instalador: `CodexBar-<version>-Setup.exe`
- Portable: `CodexBar-<version>-portable.exe`
- Sumas de verificación: cada versión incluye archivos `.sha256`

La distribución por Winget está aprobada a través de [microsoft/winget-pkgs](https://github.com/microsoft/winget-pkgs/tree/master/manifests/f/Finesssee/Win-CodexBar). Las nuevas versiones pueden tardar un poco en aparecer porque cada actualización de Winget está vinculada a una URL de versión específica y su hash de instalador.

## Primer uso

1. Inicia **CodexBar** desde el Menú Inicio o el ejecutable portable.
2. Haz clic en el icono de la bandeja para abrir el panel de uso.
3. Abre **Configuración → Proveedores**.
4. Habilita los proveedores que uses.
5. Agrega el tipo de credencial correspondiente: inicio de sesión OAuth/dispositivo, clave API, cookies del navegador, inicio de sesión CLI local o cuenta de token.

Para Claude, se prefieren las cookies del navegador/sessionKey porque coinciden con el uso que muestra la página de configuración de Claude. OAuth y CLI permanecen como alternativas. Para proveedores basados en CLI como Codex y Gemini, inicia sesión primero con la CLI del proveedor.

## Última versión

**v0.37.6** agrega modo de ventana con controles de título, visibilidad en la barra de tareas, escalado de pantalla y compatibilidad `es-MX`. También separa la CLI instalada como `codexbar-cli.exe` para que la salida estándar funcione correctamente en scripts.

Consulta el historial completo en [CHANGELOG.md](CHANGELOG.md).

## Proveedores compatibles

<details>
<summary>Matriz de proveedores</summary>

| Proveedor | Autenticación | Seguimiento |
|--------|----------|----------|
| Codex | OAuth / CLI | Sesión, Semanal, Créditos |
| Claude | Cookies / OAuth alternativo / CLI alternativo | Sesión (5h), Semanal |
| Cursor | Cookies | Plan, Uso, Facturación |
| Factory | Cookies | Uso |
| Gemini | gcloud OAuth | Cuota |
| Copilot | GitHub Device Flow / gh CLI / token heredado | Uso del plan, Chat |
| Antigravity | LSP local | Uso, Cuotas por modelo |
| z.ai | Token API | Cuota |
| MiniMax | API / Cookies | Uso, Resumen de facturación |
| Kiro | Cookies / CLI | Créditos mensuales, Excedente |
| Vertex AI | gcloud OAuth | Costo |
| Augment | Cookies | Créditos |
| OpenCode | Configuración local | Uso |
| Kimi | Cookies | Tasa 5h, Semanal |
| Kimi K2 | Clave API | Créditos |
| Amp | Cookies | Uso |
| Warp | Configuración local | Uso |
| Ollama | Cookies / Clave API | Uso, Modelos en la nube, Ventanas de ritmo |
| Azure OpenAI | Clave API | Despliegue |
| T3 Chat | Cookies / cURL | Base, Excedente |
| OpenRouter | Clave API | Créditos |
| JetBrains AI | Configuración local | Uso |
| Alibaba | Cookies | Uso |
| Alibaba Token Plan | Cookies | Créditos del plan de tokens, Fecha de reinicio |
| NanoGPT | Clave API | Créditos |
| Infini | Clave API | Sesión, Semanal, Cuota |
| Perplexity | Cookies | Créditos, Plan |
| Abacus AI | Cookies | Créditos |
| Mistral | Cookies | Facturación, Uso |
| OpenCode Go | Cookies | Uso, Saldo Zen |
| Kilo | Clave API / CLI | Uso |
| Codebuff | Clave API / Configuración local | Créditos, Semanal |
| DeepSeek | Clave API | Saldo, Resúmenes de uso, Costo |
| Windsurf | Caché local | Diario, Semanal |
| Manus | Cookies | Créditos, Refrescar créditos |
| Xiaomi MiMo | Cookies | Saldo, Plan de tokens |
| Doubao | Clave API | Límites de solicitudes |
| Command Code | Cookies | Créditos mensuales, Créditos comprados |
| Crof | Clave API | Créditos, Cuota de solicitudes |
| StepFun | Token Oasis | 5h, Semanal, Refresco de token |
| Venice | Clave API | Saldo USD / DIEM |
| OpenAI | Admin API / Clave API | Uso, Solicitudes, Costo con ámbito de proyecto, Saldo de créditos |
| Grok | Cookies / auth.json | Facturación |
| ElevenLabs | Clave API | Créditos de suscripción, Slots de voz |
| Deepgram | Clave API | Uso del proyecto |
| Groq | Clave API | Métricas empresariales |
| LLM Proxy | Clave API | Estadísticas de cuota |

</details>

## Compilar desde el código fuente

```powershell
# Requisitos previos: Node.js + pnpm. Rust y MinGW se instalan automáticamente cuando se necesitan.
git clone https://github.com/Finesssee/Win-CodexBar.git
cd Win-CodexBar
.\scripts\dev.ps1
```

Opciones útiles de desarrollo:

```powershell
.\scripts\dev.ps1 -Release      # compilación optimizada
.\scripts\dev.ps1 -SkipBuild    # relanzar la última compilación
```

Ejemplos de CLI:

```bash
codexbar --help
codexbar diagnose --pretty
codexbar usage -p claude
codexbar usage -p all
codexbar cost -p codex
```

Las compilaciones con instalador incluyen `codexbar.exe` como la CLI de consola y `codexbar-desktop.exe` como la aplicación de bandeja. Los accesos directos del Menú Inicio lanzan la aplicación de escritorio; los comandos de terminal usan `codexbar.exe`.

## Compilaciones de versión

Para compilaciones de versión locales en Windows, usa el constructor de versiones en caché:

```powershell
.\scripts\windows-release-build.ps1 -Ref v0.37.6 -SmokeInstall
```

El script compila el binario real de versión de Tauri más la CLI de consola, verifica las dependencias firmadas del instalador, empaqueta con Inno Setup, genera los archivos de instalador/portable, genera los archivos SHA-256 complementarios y puede ejecutar una prueba de instalación/desinstalación silenciosa.

Más notas sobre automatización de versiones en [docs/release/ci-cd.md](docs/release/ci-cd.md).

## Privacidad

- **En el dispositivo por defecto**: los datos del proveedor se leen desde rutas locales conocidas o APIs del proveedor que configures.
- **Cookies opcionales**: la extracción de cookies del navegador solo se ejecuta para los proveedores que habilites.
- **Secretos protegidos**: las claves API, cookies manuales y cuentas de token usan la capa de archivo seguro; en Windows se utiliza DPAPI con ámbito de usuario cuando está disponible.
- **Diagnóstico seguro**: los diagnósticos exponen solo metadatos de proveedor/fuente/estado, nunca cookies sin procesar, claves API, tokens de portador ni valores OAuth.
- **Actualizaciones verificadas**: las descargas del instalador requieren un resumen SHA-256 de GitHub y se vuelven a verificar inmediatamente antes de aplicar.

## Documentación

| Tema | Enlace |
|------|------|
| Compilar desde el código fuente | [docs/BUILDING.md](docs/BUILDING.md) |
| Configuración de WSL y consejos de autenticación | [docs/WSL.md](docs/WSL.md) |
| Detalles de cookies del navegador | [docs/COOKIES.md](docs/COOKIES.md) |

## Créditos

- Aplicación original para macOS: [steipete/CodexBar](https://github.com/steipete/CodexBar) por Peter Steinberger
- Inspirado por [ccusage](https://github.com/ryoppippi/ccusage) para el seguimiento de costos

## Licencia

MIT, igual que el CodexBar original.
