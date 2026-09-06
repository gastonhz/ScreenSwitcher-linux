# Screen Switcher (Linux / COSMIC)

Selector de **perfiles de monitores** + **control de brillo DDC/CI**, como applet
nativo del panel de **COSMIC** (Pop!_OS 24.04+). Cambia entre configuraciones de
pantallas (encender/apagar, resolución, rotación, posición, monitor principal)
con un click.

Hecho en **Rust + libcosmic** (el mismo toolkit del escritorio COSMIC). Sin
Electron, sin demonios propios: el ciclo de vida lo maneja `cosmic-panel`.

> Versión Linux del [Screen Switcher para Windows](https://github.com/gastonhz/ScreenSwitcher-Windows-Native).

## Cómo funciona

- **Perfiles**: cada uno describe el estado completo de las pantallas
  (encendida/apagada, modo, posición, rotación, primario) en
  [`profiles.kdl`](profiles.kdl), referenciando cada salida por su nombre de
  conector (`HDMI-A-1`, `DP-2`, `DP-1`).
- **Motor**: habla con el compositor a través de la CLI nativa **`cosmic-randr`**
  (el protocolo de output-management de COSMIC).
- **Brillo**: DDC/CI vía el crate `ddc-hi`, con acceso directo a `/dev/i2c-*`.

El motor aplica cada perfil en fases: primero enciende lo que falta (nunca quedan
0 pantallas), después setea modo/posición/rotación salida por salida, apaga lo que
sobra, marca el primario (compat Xwayland) y relee el estado para corregir
posiciones si el compositor corrió algo. Si el refresco declarado para una salida
ya no figura en su lista de modos, reintenta esa salida sin fijar refresco en vez
de abortar: un modo desactualizado degrada la calidad, pero no deja el layout a
medio aplicar. Cuando eso pasa, el popup lo avisa.

## Los 7 perfiles

Referenciados por conector en [`profiles.kdl`](profiles.kdl):

| Perfil | Descripción |
|--------|-------------|
| `escritoriodoble-vertical` | Principal + secundario en vertical · TV apagada |
| `escritoriodoble-horizontal` | Principal + secundario en horizontal · TV apagada |
| `tres-pantallas-horizontal` | Las 3 pantallas en horizontal |
| `escritoriodoble-y-tv` | Principal + secundario vertical + TV |
| `solo-tv` | Solo la TV (como principal) |
| `tv-principal-y-display10` | TV principal + monitor (para juegos) |
| `solo-principal` | Solo el monitor principal |

> ⚠️ Los perfiles están ajustados a mi setup. Si clonás esto, corré
> `cosmic-randr list` y reemplazá los conectores/resoluciones en
> `~/.config/screen-switcher/profiles.kdl`. El nombre de conector es estable
> mientras no cambies el cable de puerto físico.
>
> Nota: las posiciones se normalizan para que la salida de más a la izquierda
> quede en `x=0`; no se usan coordenadas negativas.

### Reajustar los perfiles al hardware: `retune-profiles.py`

Un cambio de driver, de adaptador o de cable de puerto puede dejar los perfiles
desincronizados del hardware, y el síntoma es confuso: el perfil se aplica "casi
bien", con alguna pantalla en su posición nueva y otra en la vieja.

```bash
./retune-profiles.py           # diagnostica y muestra el diff, sin escribir
./retune-profiles.py --write   # aplica (deja un .bak)
```

Lee `cosmic-randr list --kdl`, reidentifica cada monitor por su identidad EDID
(los atributos `match-make` / `match-model` / `match-serial`, que agrega solo la
primera vez) en vez de por el nombre del conector, y corrige los `connector=` y
`refresh=` que quedaron mal. Preserva comentarios y formato: sólo toca valores.

La corrección del refresco va en los dos sentidos. Si el declarado no está
disponible, lo baja al más cercano y anota el deseado en `refresh-want`; si ese
refresco reaparece más adelante, la siguiente corrida lo restaura y borra el
marcador. Por defecto trabaja sobre `~/.config/screen-switcher/profiles.kdl`;
con `--file profiles.kdl`, sobre la copia del repo que va embebida en el binario.

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
        ├─ src/app.rs            estado + mensajes + popup
        └─ src/view.rs           tiles + diagramas + sliders
```

## Verificación no destructiva

- `cosmic-randr list --kdl` — leer el estado nunca cambia nada.
- `./retune-profiles.py` sin `--write` — compara los perfiles contra el hardware
  y muestra el diff sin tocar el archivo.
- `cosmic-applet-screen-switcher --check` — carga los perfiles, lee el estado
  real y dice qué perfil está activo. No abre UI ni cambia nada.
- `cargo check` / `cargo clippy` — chequeo estático.
- Aplicar un perfil de verdad reordena las pantallas en vivo: eso lo dispara el
  usuario desde la UI, no las pruebas.

> ⚠️ `cosmic-randr mode <salida> <W> <H> --test ...` **no** sirve como dry-run.
> El `--help` dice que prueba la configuración sin aplicarla, pero en la versión
> de COSMIC probada **aplica el cambio igual**. Para validar un modo sin mover
> nada, comparalo contra la lista de modos de `cosmic-randr list --kdl`.

## Notas y limitaciones

- **"Monitor principal"** en Wayland es un concepto débil: `cosmic-randr
  xwayland --primary` solo afecta apps X11/Xwayland (juegos, etc.). La posición
  del panel de COSMIC se configura aparte, en Configuración → Panel.
- El apply **no es atómico**: son varias llamadas seguidas a `cosmic-randr`. La
  fase 5 (releer y corregir) compensa; a futuro se puede migrar a
  `cosmic-randr kdl` (apply atómico por stdin).
- **DDC/CI**: si un slider no aparece, revisá que DDC/CI esté activado en el
  OSD del monitor, y que la regla udev esté instalada. Las TVs por HDMI suelen
  no responder.
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
