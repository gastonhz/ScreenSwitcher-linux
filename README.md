# Screen Switcher (Linux / COSMIC)

Port a Linux del [Screen Switcher de Windows](https://github.com/gastonhz/ScreenSwitcher-Windows-Native): selector
de **perfiles de monitores** + **control de brillo DDC/CI**, como applet nativo del
panel de **COSMIC** (Pop!_OS 24.04+). Cambia entre configuraciones de pantallas
(encender/apagar, resolución, rotación, posición, monitor principal) con un click.

Hecho en **Rust + libcosmic** (el mismo toolkit del escritorio COSMIC). Sin
Electron, sin demonios propios: el ciclo de vida lo maneja `cosmic-panel`.

## Cómo funciona

| Windows (original) | Linux (este port) |
|---|---|
| API CCD (`SetDisplayConfig`) vía módulo DisplayConfig | CLI nativa **`cosmic-randr`** (protocolo de output-management de COSMIC) |
| HW ID estable (`GSM57C7`…) en `profiles.psd1` | Nombre de conector (`HDMI-A-1`, `DP-2`, `DP-1`) en `profiles.kdl` |
| Brillo DDC/CI vía módulo MonitorConfig | Brillo DDC/CI vía crate `ddc-hi` (acceso directo a `/dev/i2c-*`) |
| Bandeja del sistema (NotifyIcon + mutex) | Applet del panel COSMIC (instancia única y autostart incluidos) |
| Ventana WPF dark con tiles + diagramas | Popup libcosmic con tiles + diagramas (misma paleta de roles) |

El motor aplica cada perfil en fases, como el original: primero enciende lo que
falta (nunca quedan 0 pantallas), después setea modo/posición/rotación salida por
salida, apaga lo que sobra, marca el primario (compat Xwayland) y relee el estado
para corregir posiciones si el compositor corrió algo.

## Los 7 perfiles

Los mismos de Windows, keyados por conector en [`profiles.kdl`](profiles.kdl):

| Perfil | Descripción |
|--------|-------------|
| `escritoriodoble-vertical` | Principal + secundario en vertical · TV apagada |
| `escritoriodoble-horizontal` | Principal + secundario en horizontal · TV apagada |
| `tres-pantallas-horizontal` | Las 3 pantallas en horizontal |
| `escritoriodoble-y-tv` | Principal + secundario vertical + TV |
| `solo-tv` | Solo la TV (como principal) |
| `tv-principal-y-display10` | TV principal + monitor (para juegos) |
| `solo-principal` | Solo el monitor principal |

> ⚠️ Igual que en Windows, los perfiles están atados a MIS monitores. Si clonás
> esto, corré `cosmic-randr list` y reemplazá los conectores/resoluciones en
> `~/.config/screen-switcher/profiles.kdl`. A diferencia del `DisplayId` de
> Windows, el nombre de conector en Linux **sí es estable** mientras no cambies
> el cable de puerto físico.
>
> Nota: las posiciones se normalizan para que la salida de más a la izquierda
> quede en `x=0` (acá no hay coordenadas negativas como en Windows). La TV, que
> en Windows iba en `X=-1920`, acá va en `x=0` con el resto corrido a la derecha.

## Requisitos

- Pop!_OS 24.04+ con escritorio **COSMIC** (usa `cosmic-randr`, ya incluido).
- Toolchain de Rust para compilar: `curl https://sh.rustup.rs -sSf | sh`
- Para el brillo DDC/CI sin root: la regla udev de `setup-brillo-ddc.sh` (una vez).

## Instalación

```bash
./install.sh                    # compila e instala (binario, .desktop, perfiles)
sudo ./setup-brillo-ddc.sh      # (una vez) permisos DDC/CI para el brillo
```

Después: **Configuración → Escritorio → Panel → Configurar applets → agregar
"Screen Switcher"**. El applet queda en el panel, arranca con la sesión y es de
instancia única (todo eso lo garantiza el panel, no hace falta autostart aparte).

## Uso

- **Click en el ícono del panel** → abre el popup.
- **Click en un tile** → aplica ese perfil. El tile del perfil activo queda
  resaltado con el color de acento.
- **Sliders de brillo** → ajustan cada monitor en vivo (el canal interno
  conserva solo el último valor, que hace de *debounce*; los monitores que no
  responden DDC/CI —la TV— se omiten solos).
- Los perfiles se releen de `~/.config/screen-switcher/profiles.kdl` cada vez
  que se abre el popup: editar el archivo no requiere recompilar ni reiniciar.

## Arquitectura

```
cosmic-applet-screen-switcher    (applet libcosmic, 1 binario)
        ├─ src/display.rs        motor: aplica perfiles vía `cosmic-randr` (fases)
        ├─ profiles.kdl          los 7 perfiles (por conector) — editable en caliente
        ├─ src/brightness.rs     brillo DDC/CI (ddc-hi) en una Subscription
        ├─ src/profiles.rs       parseo KDL de los perfiles
        ├─ src/app.rs            estado + mensajes + popup (rol de Tray.ps1)
        └─ src/view.rs           tiles + diagramas + sliders (rol del WPF)
```

## Verificación no destructiva (misma filosofía que el original)

- `cosmic-randr list --kdl` — leer estado nunca cambia nada.
- `cosmic-randr mode <salida> <W> <H> --test ...` — dry-run de geometría sin aplicar.
- `cargo check` / `cargo clippy` — chequeo estático.
- Aplicar un perfil de verdad reordena las pantallas en vivo: eso lo dispara el
  usuario desde la UI, no las pruebas.

## Notas y limitaciones

- **"Monitor principal"** en Wayland es un concepto débil: `cosmic-randr
  xwayland --primary` solo afecta apps X11/Xwayland (juegos, etc.). La posición
  del panel de COSMIC se configura aparte, en Configuración → Panel.
- El apply **no es atómico** como `Use-DisplayConfig` en Windows: son varias
  llamadas seguidas a `cosmic-randr`. La fase 5 (releer y corregir) compensa;
  a futuro se puede migrar a `cosmic-randr kdl` (apply atómico por stdin).
- **DDC/CI**: si un slider no aparece, revisá que DDC/CI esté activado en el
  OSD del monitor, y que la regla udev esté instalada. Las TVs por HDMI suelen
  no responder (igual que en Windows).
- COSMIC es un escritorio joven: el protocolo de output-management puede
  cambiar entre versiones. Este applet habla con la CLI `cosmic-randr` del
  sistema justamente para quedar acoplado a la versión instalada del compositor.
- **Bug conocido de COSMIC** ([cosmic-epoch#3163](https://github.com/pop-os/cosmic-epoch/issues/3163)):
  al cambiar la topología de pantallas, la barra y el dock pueden no reaparecer
  (cosmic-panel no recrea sus superficies). Rescate manual:
  ```bash
  pkill -x cosmic-panel   # cosmic-session lo respawnea en ~1s
  ```
  Además, si la barra superior está anclada a una salida específica
  (`~/.config/cosmic/com.system76.CosmicPanel.Panel/v1/output` = `Name("...")`),
  no va a aparecer en los perfiles que apagan esa salida; es config de COSMIC,
  no de la app. A propósito no se automatiza ningún workaround acá: se espera
  el fix upstream.

## Créditos

- Estructura de applet inspirada en
  [cosmic-ext-applet-external-monitor-brightness](https://github.com/cosmic-utils/cosmic-ext-applet-external-monitor-brightness)
  (GPL-3.0), de la comunidad COSMIC Utils.
- Versión original para Windows: [ScreenSwitcher-Windows-Native](../ScreenSwitcher-windows/).
