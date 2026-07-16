# Reporte v2: cosmic-screenshot falla en Pop!_OS — causa raíz identificada

> **v2** — reemplaza la versión anterior. Se encontró el evento raíz y se corrigió el
> mecanismo descrito en v1 (que solo aplicaba a uno de los dos modos de falla).

## Para qué es esto

Bug: la tecla Impr Pant no saca capturas en Pop!_OS/COSMIC. Se identificó el evento
raíz pero **no el disparador**.

Sospechoso secundario: `cosmic-applet-screen-switcher`, un applet que el usuario
desarrolló con Claude Code (`~/.local/bin/`). **No está probado que tenga nada que
ver** — ver sección correspondiente.

---

## Entorno

- **SO:** Pop!_OS 24.04 LTS, COSMIC, Wayland
- **Kernel:** 7.0.11-76070011-generic
- **GPU:** NVIDIA RTX 3060 (GA106), driver 580.159.03, `nvidia-drm.modeset=1`
- **Monitores:**
  - `HDMI-A-1` → E2340
  - `DP-1` → TV Samsung (se habilita/deshabilita seguido)
  - `DP-2` → LG (montado en vertical)
  - `DP-3` → libre
- **Usuario:** `gaston` (uid 1000)

---

## Síntoma

Se aprieta Impr Pant y no aparece la herramienta de captura ni se guarda archivo.
A veces las pantallas parpadean (el portal llega a capturar frames) pero la UI nunca
sale.

Según el modo de falla, además quedan procesos colgados que se acumulan:
```
gaston   2533331  /bin/sh -c cosmic-screenshot     S<+
gaston   2533332  cosmic-screenshot                S<+
```

No es aleatorio: funciona hasta que el portal falla, y después no anda más.

---

## Arquitectura (contexto)

`cosmic-screenshot` no captura por sí mismo. Manda una petición D-Bus a
`org.freedesktop.impl.portal.desktop.cosmic` (proceso `xdg-desktop-portal-cosmic`).
El portal captura los frames y dibuja la UI interactiva. `cosmic-screenshot` espera
la respuesta.

El portal se levanta por activación D-Bus como unidad transitoria:
`dbus-:1.1-org.freedesktop.impl.portal.desktop.cosmic@N.service`, con `N` incremental.

---

## ⭐ EVENTO RAÍZ (reproducción del 16/jul, portal PID 2517)

```
jul 16 03:02:55  xdg-desktop-portal-cosmic[2517]:
  cosmic_corner_radius_layer_v1#680: error 1: CosmicCornerRadiusLayerV1 {
    id: ObjectId(cosmic_corner_radius_layer_v1@680[28], 5473),
    version: 2, data: Some(Any { .. }),
    handle: WeakHandle { handle: WeakInnerHandle[rs] { .. } }
  } corner radius too large
```

**El portal le pide al compositor un corner radius más grande de lo permitido en el
protocolo `cosmic_corner_radius_layer_v1`. El compositor rechaza con `error 1`. En
Wayland un error de protocolo es fatal: mata la conexión del cliente.**

Aparece **exactamente 1 vez** en todo el log. Es el evento raíz.

### Amplificación
Después de eso, el portal entra en loop reintentando sobre el objeto ya muerto:
```
Protocol error 1 on object cosmic_corner_radius_layer_v1@680      × 8838
```
**Los 8838 errores son sobre el MISMO objeto `@680`** — es un solo evento amplificado
por un retry loop, no 8838 fallas distintas. ~8838 líneas en ~50 segundos.

---

## Los DOS modos de falla (importante)

Se observaron dos firmas distintas que llevan al mismo síntoma. **No confundirlas.**

### Modo A — panic + Mutex envenenado (sesión del 15/jul, PID 1208032)
```
jul 15 23:19:35  thread '<unnamed>' panicked at src/wayland/mod.rs:582:61:
                 called `Option::unwrap()` on a `None` value
   ↓ (el Mutex queda envenenado)
jul 16 02:11:49  thread 'tokio-rt-worker' panicked at src/wayland/mod.rs:287:35:
                 called `Result::unwrap()` on an `Err` value: PoisonError { .. }
                 (cientos de veces)
```
**Mecánica (Rust):** si un thread panickea con un `Mutex` tomado, el Mutex queda
envenenado y todo `.lock().unwrap()` posterior devuelve `Err` y panickea. Permanente
para la vida del proceso.

Panickea un thread worker de tokio, **no el main** → **el proceso sobrevive** →
conserva el nombre D-Bus → D-Bus nunca lo relanza → `cosmic-screenshot` cuelga para
siempre esperando una respuesta que no llega → los procesos se acumulan.

**En este modo, `pkill -f xdg-desktop-portal-cosmic` SÍ arregla** (verificado).

### Modo B — corner radius → conexión Wayland muerta (sesión del 16/jul, PID 2517)
```
jul 16 03:02:55  corner radius too large → Protocol error 1
jul 16 03:02:55  Error: EventLoop(ExitFailure(1))
jul 16 03:02:55  cosmic@0.service: Main process exited, code=exited, status=1/FAILURE
jul 16 03:03:14  cosmic@1.service arranca (PID 12922)
jul 16 03:03:22  PID 12922: SCTK dispatch error: Connection reset by peer (os error 104)
                 Error trying to flush the wayland display: Connection reset by peer
jul 16 03:03:22  cosmic@1.service: Failed with result 'exit-code'    ← murió en 8 segundos
jul 16 03:03:47  cosmic@2.service arranca (PID 12987)
```

**En este modo el proceso MUERE y D-Bus lo relanza — y la instancia nueva vuelve a
morir.** `@0` → `@1` (8 segundos de vida) → `@2`.

**En este log NO hay ni un solo panic ni PoisonError.** Cero `panicked at`.

**Implicancia crítica:** si el portal se relanza solo y sigue fallando, **la causa es
algo persistente** (config en disco, estado del layout de displays), no un mutex
envenenado en memoria. **Y el workaround del `pkill` NO sirve en este modo** — el
portal ya se está matando y relanzando solo.

---

## ⭐ Pista principal: la config de tema está incompleta

En **cada** arranque del portal, sin excepción:
```
ERROR cosmic::theme] error loading system dark theme
  error=GetKey("list_button", Os { code: 2, kind: NotFound, message: "No such file or directory" })
```

**El corner radius sale del tema de COSMIC.** Si la config de tema está incompleta y
el portal cae a un default inválido (o a un valor demasiado grande para la superficie
que está creando), se explica el `corner radius too large`.

Encaja con el Modo B: **cada instancia fresca arranca con el mismo tema roto y muere
por lo mismo**. Es la única explicación observada que da cuenta de la persistencia
tras el respawn.

**HIPÓTESIS NO VERIFICADA.** Falta:
- Encontrar dónde vive la config de tema y qué le falta:
  ```bash
  ls ~/.config/cosmic/ | grep -iE 'theme'
  find ~/.config/cosmic/ -ipath '*[Tt]heme*' -type f
  find ~/.config/cosmic/ -iname '*corner*' -o -iname '*radi*'
  ```
- Ver si hay una clave de corner radius y qué valor tiene.
- Probar un reset de la config de tema (**con backup**) y ver si el bug desaparece.
- Contexto relevante: el usuario migró desde otra instalación y tiene configs
  posiblemente parciales/heredadas. También hay un error viejo sin resolver:
  `Failed to set xwayland primary output: The provided output wasn't known to Xwayland`.

---

## Cronología completa de la reproducción (16/jul, post-reboot)

```
02:48:02  arranca el portal @0 (PID 2517)
          ERROR cosmic::theme: GetKey("list_button", NotFound)     [en cada arranque]
02:50:01  BASELINE: 3 instancias del applet (2333, 2364, 2382)
          DRM: DP-1 connected, DP-2 connected, DP-3 disconnected, HDMI-A-1 connected
          0 procesos cosmic-screenshot
02:51:55  captura #1 → OK
02:53:09  ibus-portal@0 falla (status=1) — ruido, no relacionado
~02:53    capturas #2, #3 → OK
02:54:56  usuario DESHABILITA la TV (quedan 2 monitores)
          captura → OK                                    ← ¡el toggle NO rompió nada!
~02:55    Zen browser crashea (¿relacionado? sin datos)
          captura → OK
02:56:26  usuario RE-HABILITA la TV (3 monitores)
02:56:33  ibus-portal@1 falla — ruido
02:57:06  captura → OK
02:57:44  abre vscode, navega archivos con cosmic-files
02:58:28  WARN iced_futures::subscription::tracker:
          Error sending event to subscription: TrySendError { kind: Full }   × 34
          ^ backpressure en el canal de eventos; el portal seguía funcionando
02:58:38  flatpak-portal arranca
02:58:47  captura → OK                                    ← última captura buena
02:59:17  usuario DESHABILITA la TV de nuevo
02:59:44  xdg-desktop-por[2362] (portal GTK, otro proceso):
          "Failed to associate portal window with parent window"
03:00:42  usuario baja un .deb de vscode y lo instala por la tienda de COSMIC
   ...    (sin eventos en el log entre 02:59:44 y 03:02:55)
03:02:55  ⭐ corner radius too large → Protocol error 1 → EventLoop(ExitFailure(1))
          @0 muere. Flood de 8838 protocol errors sobre @680.
03:03:14  @1 arranca (PID 12922)
03:03:22  @1 muere: Connection reset by peer
03:03:45  usuario reporta: "spameé 3 o 4 veces el botón y falló, ya no anda más.
          no sucede nada o simplemente parpadean las pantallas"
          ps: solo 2 instancias del applet (2333, 2382) — LA 2364 DESAPARECIÓ
          ps: 0 procesos cosmic-screenshot colgados (a diferencia del modo A)
03:03:47  @2 arranca (PID 12987)
          Screenshot output count mismatch: 0 != 2
          Screenshot images: ["HDMI-A-1", "DP-2"]
          ^ el log termina acá (Ctrl+C). NO SE SABE si @2 quedó sano.
```

### Ventana del disparador
Entre la última captura OK (02:58:47) y el fallo (03:02:55) el usuario hizo:
1. Deshabilitar la TV (02:59:17)
2. Instalar vscode por la tienda de COSMIC (03:00:42)

**PERO:** ya había deshabilitado la TV a las 02:54:56 y el screenshot siguió andando.
**El toggle de displays solo no alcanza como explicación.**

---

## Descartado con evidencia

### El "output count mismatch" NO es el disparador
```
ERROR Screenshot output count mismatch: 0 != 3
WARN  Screenshot outputs: []
WARN  Screenshot images: ["HDMI-A-1", "DP-1", "DP-2"]
```
Parece grave (lista interna de outputs vacía con 3 imágenes) y fue la primera
sospecha. **Es benigno:** un portal recién nacido lo emite en su primera captura **y
la captura funciona igual**. Aparece siempre. No perder tiempo acá.

### `TrySendError { kind: Full }` es benigno
34 ocurrencias a las 02:58:28 (`iced_futures::subscription::tracker`). Es
backpressure del canal de eventos. El portal siguió funcionando 4 minutos más
(captura OK a las 02:58:47). **No confundir** con `TrySendError { kind: Disconnected }`
(1 sola ocurrencia), que sí es la conexión muerta.

### El toggle de displays solo no rompe
Verificado: deshabilitar la TV a las 02:54:56 → captura posterior OK.

### Matar solo `cosmic-screenshot` no sirve
Verificado empíricamente (modo A): limpia los colgados pero los siguientes tecleos se
cuelgan igual. **El proceso que importa es el portal.**

### El error de tema aparece siempre (pero ver "Pista principal")
`GetKey("list_button", NotFound)` sale en cada arranque, incluso cuando todo anda.
Por sí solo no impide el funcionamiento — **pero es evidencia de que la config de
tema está incompleta**, que es la pista principal.

### No es ninguno de los bugs upstream más reportados
Los issues conocidos de `cosmic-screenshot` (pop-os/cosmic-screenshot #103, #209, #3,
foro de EndeavourOS) son crashes inmediatos con `PortalNotFound` / `UnknownMethod` y
core dumped. Acá el fallo es distinto.
El más parecido: **pop-os/cosmic-epoch#887** ("funciona un rato después de loguearse y
después el atajo deja de hacer nada").

---

## Sobre el applet cosmic-applet-screen-switcher

```
gaston  2333  /home/gaston/.local/bin/cosmic-applet-screen-switcher
gaston  2364  /home/gaston/.local/bin/cosmic-applet-screen-switcher   ← desapareció durante la sesión
gaston  2382  /home/gaston/.local/bin/cosmic-applet-screen-switcher
```

Applet propio del usuario (desarrollado con Claude Code).

**Estado: ni implicado ni exculpado.**

- **No aparece en el log**, pero el `grep` del usuario filtraba por
  `portal|panic|Protocol error|Poison` — el applet no matchea ninguno. **Su ausencia
  no prueba nada.**
- El pedido inválido (`corner radius too large`) **lo manda el portal**, no el applet.
- **Arrancan 3 instancias** en cada boot (confirmado post-reboot: 2333, 2364, 2382 a
  las 02:48). Eso ya es un bug propio, independiente de esto.
- Una instancia (2364) **murió sola** durante la sesión. Sin datos de por qué.
- Los applets **son layer surfaces**, y el error es sobre un protocolo de layer
  surface (`cosmic_corner_radius_layer_v1`). Es una coincidencia temática, no una
  prueba.

### Qué revisar en el applet
- ¿Por qué arrancan 3 instancias? ¿Cómo se lanza? ¿Hay leak?
- ¿Usa `cosmic_corner_radius_layer_v1` o algún protocolo de layer shell?
- ¿Setea algún corner radius?
- ¿Qué hace para "switchear" pantallas? ¿`cosmic-randr`, APIs de COSMIC, escribe
  config, manipula outputs por Wayland?
- ¿Toca la config de tema de COSMIC?

---

## Qué investigar (por prioridad)

1. **La config de tema** — la pista más fuerte. Ver sección "Pista principal".
   Encontrarla, ver si le falta el corner radius o si tiene un valor absurdo, probar
   reset con backup.
2. **El código upstream** — `pop-os/xdg-desktop-portal-cosmic`. Buscar dónde se crea
   el `cosmic_corner_radius_layer_v1` y de dónde saca el radio. Y en `cosmic-comp`,
   la validación que emite `corner radius too large` (para saber contra qué valida:
   ¿tamaño de la superficie? ¿un máximo fijo?).
   Si valida contra el tamaño de la superficie, una superficie de 0px haría que
   cualquier radio sea "demasiado grande" → conectaría con los outputs.
3. **`src/wayland/mod.rs` líneas 582 y 287** (modo A) — el fix correcto sería no
   hacer `unwrap()` del `Option` en 582 y manejar el `PoisonError` en 287.
4. **Confirmar/descartar el applet** — matar las 3 instancias y ver si el bug tarda
   más en aparecer.
5. **RUST_BACKTRACE** — lanzar el portal a mano con `RUST_BACKTRACE=full` para el
   modo A. Hay que matar el que corre y evitar que D-Bus lo relance.
6. **Reportar upstream** en `pop-os/xdg-desktop-portal-cosmic` — el
   `corner radius too large` es concreto y accionable.

---

## Workaround

```bash
pkill -f xdg-desktop-portal-cosmic
pkill -f cosmic-screenshot     # limpiar huérfanos
```

**Funciona solo en el modo A** (portal vivo y envenenado). Verificado.
**No sirve en el modo B** — ahí el portal ya muere y se relanza solo, y la instancia
nueva vuelve a fallar.

---

## Si hay que construir la herramienta

### Lo que NO funciona
- **Envolver `cosmic-screenshot` con `timeout`**: el proceso legítimamente se queda
  vivo mientras el usuario interactúa con la UI. Un timeout mataría el uso normal.
- **Un watcher que mate el portal al detectar el fallo**: sirve para el modo A, pero
  **es inútil en el modo B** — el portal ya se mata solo y la instancia nueva falla
  igual.

### Conclusión
**Conviene arreglar la causa antes que automatizar el workaround.** Si la pista del
tema se confirma, el fix es de config y la herramienta no hace falta.

Si aun así se quiere la herramienta (para el modo A):
- Daemon de usuario (systemd user service) que siga el journal filtrando por
  identificador de proceso (`xdg-desktop-portal-cosmic`, **no** por nombre de unidad
  — es dinámico: `...cosmic@N.service`).
- Al detectar `panicked at ... mod.rs:582` o el primer `PoisonError`, matar el portal
  y limpiar los `cosmic-screenshot` huérfanos.
- Modelo de referencia que ya funciona en esta máquina: el daemon del botón de mute
  del Corsair HS70 (Python + systemd user service, escucha un evento y actúa).

---

## Comandos de diagnóstico

```bash
# PID e instancia actual del portal
pgrep -af xdg-desktop-portal-cosmic
systemctl --user list-units | grep -i portal
busctl --user list | grep -i portal

# Log completo de la vida de un proceso (buscar el PRIMER evento raro)
journalctl --user _PID=<PID> --no-pager

# Watcher en vivo (lo más útil para pescar el disparador)
journalctl --user -f | grep -iE 'portal|panic|Protocol error|Poison|corner'

# Filtrar el flood para ver solo los eventos únicos
journalctl --user -f | grep -iE 'portal|panic|corner' | grep -v 'Protocol error 1 on object'

# Procesos colgados
ps aux | grep cosmic-screenshot

# Conectores DRM
for p in /sys/class/drm/card*-*/status; do echo "$p -> $(cat $p)"; done

# Config de tema (la pista principal)
find ~/.config/cosmic/ -ipath '*[Tt]heme*' -type f
find ~/.config/cosmic/ -iname '*corner*' -o -iname '*radi*'
```

---

## Contexto del usuario (para calibrar respuestas)

Perfil técnico avanzado: sysadmin/SRE-DevOps con Kubernetes/OpenShift, formación en
infraestructura de TI, base de técnico mecánico/electricista.

Prefiere:
- Explicaciones directas y técnicamente precisas, con comandos concretos y pasos de
  verificación.
- El "por qué" además del "cómo".
- **Que se flaguee la incertidumbre explícitamente** en vez de afirmar sin verificar.
  Corrige activamente las afirmaciones infundadas — con razón.
- Si algo "antes funcionaba", tratarlo como regresión, no como mala configuración.
- Español rioplatense.


#############
No llegamos a generar la version V3 del reporte, pero si continuamos el chat. Te pasolo que le envié:



""""""""""""""
el screenshot ahora no anda. acabo de tocar la tecla y apareció por un instante el recuadro para enmarcar y se fue

gaston@pop-os:~$ ls ~/.config/cosmic/ | grep -i -E 'theme|Theme'
com.system76.CosmicTheme.Dark
com.system76.CosmicTheme.Dark.Builder
com.system76.CosmicTheme.Light
com.system76.CosmicTheme.Light.Builder
com.system76.CosmicTheme.Mode
gaston@pop-os:~$ find ~/.config/cosmic/ -ipath '*Theme*' -type f | head -30
/home/gaston/.config/cosmic/com.system76.CosmicTheme.Light/v1/accent_text
/home/gaston/.config/cosmic/com.system76.CosmicTheme.Light/v1/button
/home/gaston/.config/cosmic/com.system76.CosmicTheme.Light/v1/success_button
/home/gaston/.config/cosmic/com.system76.CosmicTheme.Light/v1/is_frosted
/home/gaston/.config/cosmic/com.system76.CosmicTheme.Light/v1/is_dark
/home/gaston/.config/cosmic/com.system76.CosmicTheme.Light/v1/gaps
/home/gaston/.config/cosmic/com.system76.CosmicTheme.Light/v1/accent_button
/home/gaston/.config/cosmic/com.system76.CosmicTheme.Light/v1/is_high_contrast
/home/gaston/.config/cosmic/com.system76.CosmicTheme.Light/v1/window_hint
/home/gaston/.config/cosmic/com.system76.CosmicTheme.Light/v1/icon_button
/home/gaston/.config/cosmic/com.system76.CosmicTheme.Light/v1/warning_button
/home/gaston/.config/cosmic/com.system76.CosmicTheme.Light/v1/destructive_button
/home/gaston/.config/cosmic/com.system76.CosmicTheme.Light/v1/accent
/home/gaston/.config/cosmic/com.system76.CosmicTheme.Light/v1/shade
/home/gaston/.config/cosmic/com.system76.CosmicTheme.Light/v1/text_tint
/home/gaston/.config/cosmic/com.system76.CosmicTheme.Light/v1/secondary
/home/gaston/.config/cosmic/com.system76.CosmicTheme.Light/v1/destructive
/home/gaston/.config/cosmic/com.system76.CosmicTheme.Light/v1/primary
/home/gaston/.config/cosmic/com.system76.CosmicTheme.Light/v1/corner_radii
/home/gaston/.config/cosmic/com.system76.CosmicTheme.Light/v1/control_tint
/home/gaston/.config/cosmic/com.system76.CosmicTheme.Light/v1/active_hint
/home/gaston/.config/cosmic/com.system76.CosmicTheme.Light/v1/success
/home/gaston/.config/cosmic/com.system76.CosmicTheme.Light/v1/warning
/home/gaston/.config/cosmic/com.system76.CosmicTheme.Light/v1/spacing
/home/gaston/.config/cosmic/com.system76.CosmicTheme.Light/v1/background
/home/gaston/.config/cosmic/com.system76.CosmicTheme.Light/v1/name
/home/gaston/.config/cosmic/com.system76.CosmicTheme.Light/v1/link_button
/home/gaston/.config/cosmic/com.system76.CosmicTheme.Light/v1/palette
/home/gaston/.config/cosmic/com.system76.CosmicTheme.Light/v1/text_button
/home/gaston/.config/cosmic/com.system76.CosmicTheme.Light/v2/button
gaston@pop-os:~$ find ~/.config/cosmic/ -iname '*corner*' -o -iname '*radi*'
/home/gaston/.config/cosmic/com.system76.CosmicTheme.Light/v1/corner_radii
/home/gaston/.config/cosmic/com.system76.CosmicPanel.Panel/v1/border_radius
/home/gaston/.config/cosmic/com.system76.CosmicTheme.Dark/v1/corner_radii
/home/gaston/.config/cosmic/com.system76.CosmicTheme.Light.Builder/v1/corner_radii
/home/gaston/.config/cosmic/com.system76.CosmicTheme.Dark.Builder/v1/corner_radii
/home/gaston/.config/cosmic/com.system76.CosmicPanel.Dock/v1/border_radius
gaston@pop-os:~$ cat /home/gaston/.config/cosmic/com.system76.CosmicTheme.Light/v1/corner_radii
(
    radius_0: (0.0, 0.0, 0.0, 0.0),
    radius_xs: (2.0, 2.0, 2.0, 2.0),
    radius_s: (8.0, 8.0, 8.0, 8.0),
    radius_m: (8.0, 8.0, 8.0, 8.0),
    radius_l: (8.0, 8.0, 8.0, 8.0),
    radius_xl: (8.0, 8.0, 8.0, 8.0),
)gaston@pop-os:~$ cat/home/gaston/.config/cosmic/com.system76.CosmicTheme.Dark/v1/corner_radiii
(
    radius_0: (0.0, 0.0, 0.0, 0.0),
    radius_xs: (2.0, 2.0, 2.0, 2.0),
    radius_s: (8.0, 8.0, 8.0, 8.0),
    radius_m: (8.0, 8.0, 8.0, 8.0),
    radius_l: (8.0, 8.0, 8.0, 8.0),
    radius_xl: (8.0, 8.0, 8.0, 8.0),
)gaston@pop-os:~$ cat/home/gaston/.config/cosmic/com.system76.CosmicTheme.Dark.Builder/v1/corner_radiii
(
    radius_0: (0.0, 0.0, 0.0, 0.0),
    radius_xs: (2.0, 2.0, 2.0, 2.0),
    radius_s: (8.0, 8.0, 8.0, 8.0),
    radius_m: (8.0, 8.0, 8.0, 8.0),
    radius_l: (8.0, 8.0, 8.0, 8.0),
    radius_xl: (8.0, 8.0, 8.0, 8.0),

gaston@pop-os:~$ date && ps aux | grep screen
jue 16 jul 2026 03:17:35 -03
gaston      2333  0.4  0.1 1382172 22700 tty1    Sl+  02:48   0:07 /home/gaston/.local/bin/cosmic-applet-screen-switcher
gaston      2382  0.0  0.1 1312036 20972 tty1    Sl+  02:48   0:01 /home/gaston/.local/bin/cosmic-applet-screen-switcher
gaston     14117  0.0  0.0  18996  2424 pts/0    S+   03:17   0:00 grep --color=auto screen

pero tampoco se acumulan procesos

gaston@pop-os:~$ ps aux | grep screen
gaston      2333  0.7  0.1 1382172 22700 tty1    Sl+  02:48   0:07 /home/gaston/.local/bin/cosmic-applet-screen-switcher
gaston      2382  0.1  0.1 1312036 20972 tty1    Sl+  02:48   0:01 /home/gaston/.local/bin/cosmic-applet-screen-switcher
gaston     13017  0.0  0.0  18996  2424 pts/0    S+   03:03   0:00 grep --color=auto screen
gaston@pop-os:~$
gaston@pop-os:~$
gaston@pop-os:~$
gaston@pop-os:~$ date && ps aux | grep screen
jue 16 jul 2026 03:17:35 -03
gaston      2333  0.4  0.1 1382172 22700 tty1    Sl+  02:48   0:07 /home/gaston/.local/bin/cosmic-applet-screen-switcher
gaston      2382  0.0  0.1 1312036 20972 tty1    Sl+  02:48   0:01 /home/gaston/.local/bin/cosmic-applet-screen-switcher
gaston     14117  0.0  0.0  18996  2424 pts/0    S+   03:17   0:00 grep --color=auto screen
gaston@pop-os:~$ date #voy a button-mashear el print screen
jue 16 jul 2026 03:18:08 -03
gaston@pop-os:~$ #saquée varias y anda ok magicamente, ya se arregló parece. no hice ningun cambio de pantallas ni nada
gaston@pop-os:~$ date
jue 16 jul 2026 03:18:40 -03
gaston@pop-os:~$ ps aux | grep screen
gaston      2333  0.3  0.1 1382172 22700 tty1    Sl+  02:48   0:07 /home/gaston/.local/bin/cosmic-applet-screen-switcher
gaston      2382  0.0  0.1 1312036 20972 tty1    Sl+  02:48   0:01 /home/gaston/.local/bin/cosmic-applet-screen-switcher
gaston     14251  0.0  0.0  18996  2424 pts/0    S+   03:18   0:00 grep --color=auto screen
gaston@pop-os:~$ ps aux | grep screen
gaston      2333  0.3  0.1 1382172 22700 tty1    Sl+  02:48   0:07 /home/gaston/.local/bin/cosmic-applet-screen-switcher
gaston      2382  0.0  0.1 1312036 20972 tty1    Sl+  02:48   0:01 /home/gaston/.local/bin/cosmic-applet-screen-switcher
gaston     14419  0.0  0.0  18996  2420 pts/0    S+   03:19   0:00 grep --color=auto screen

""""""""""""""

Y esta es la respuesta a ello de Claude:

===CLAUDECLAUDECLAUDE===

Dos hipótesis mías caen con estos datos, y una de ellas era la que puse como "pista principal" en el reporte.
1. La config de tema está bien. Mirá los valores:
radius_xl: (8.0, 8.0, 8.0, 8.0)   ← el más grande
8 píxeles. Son los defaults, perfectamente sanos. No hay ningún valor absurdo. Mi hipótesis de "el tema tiene un corner radius roto" está muerta.
Y el error GetKey("list_button", NotFound) es de otra clave: en tu find no aparece ningún list_button, pero sí hay un v1/ y un v2/ con button en cada uno. Huele a evolución de esquema (el portal lee v1 y espera una clave que se agregó después). Ruido upstream, sin relación con el corner radius.
Y esto refuerza muchísimo algo: para que 8px sea "demasiado grande", la superficie tendría que medir ≤16px. O sea que el problema no es el valor del radio — es el tamaño de la superficie. El portal está creando la layer surface con un tamaño de 0 o casi 0, y ahí cualquier radio es demasiado grande.
2. Y la corrección más importante: se arregló solo. Vos no tocaste nada — ni pantallas, ni configs, ni mataste procesos — y a las 03:18 volvió a andar.
Eso invalida mi conclusión de que la causa era persistente. Yo dije "si el portal se relanza y sigue fallando, es algo persistente en disco". Estaba equivocado: es transitorio y se auto-recupera.
Sumado a lo del tamaño de superficie, la hipótesis que mejor encaja ahora es una race condition: el portal crea la layer surface y le setea el corner radius antes de que el compositor le haya configurado su tamaño real. Si gana la carrera, anda; si la pierde, la superficie mide 0 y el compositor rechaza. Eso explica:

Por qué es intermitente sin patrón
Por qué se auto-recupera
Por qué el button-mashing a veces lo destraba (cada intento es un nuevo tiro de dados)
Por qué a las 03:17 viste el recuadro aparecer y desaparecer — la UI llegó a crearse y la conexión murió enseguida

También explica lo de los procesos: en el modo B no se acumulan porque el portal muere y cosmic-screenshot recibe un error de D-Bus y sale. Solo se acumulan en el modo A, donde el portal queda vivo y nunca responde.
Para confirmar, necesito saber si nació una instancia nueva:
bashpgrep -af xdg-desktop-portal-cosmic
systemctl --user list-units | grep -i 'portal.desktop.cosmic'
journalctl --user --since "03:15" --no-pager | grep -iE 'corner|Protocol error 1 on|cosmic@|Started dbus.*cosmic*' | grep -v 'Protocol error 1 on object' | head -20

Si a las 03:17 murió el @2 y nació un @3, queda confirmado el ciclo. Si el @2 sigue vivo desde las 03:03:47 y ahora anda, entonces el portal se recupera sin morir, y eso es otro dato.
Con eso actualizo el reporte a v3 — porque tal como está, manda a Claude Code a perseguir la config de tema, que ya sabemos que es un callejón sin salida.

===CLAUDECLAUDECLAUDE===

Y esta es la salida de lo último que me pidió:
""""""""""""""""""""""""""""""""""""""""""""""""


gaston@pop-os:~$ pgrep -af xdg-desktop-portal-cosmic
14317 /usr/libexec/xdg-desktop-portal-cosmic
gaston@pop-os:~$ systemctl --user list-units | grep -i 'portal.desktop.cosmic'
  dbus-:1.1-org.freedesktop.impl.portal.desktop.cosmic@5.service                                        loaded active running   dbus-:1.1-org.freedesktop.impl.portal.desktop.cosmic@5.service
  app-dbus\x2d:1.1\x2dorg.freedesktop.impl.portal.desktop.cosmic.slice                                  loaded active active    Slice /app/dbus-:1.1-org.freedesktop.impl.portal.desktop.cosmic
gaston@pop-os:~$ journalctl --user --since "03:15" --no-pager | grep -iE 'corner|Protocol error 1 on|cosmic@|Started dbus.*cosmic' | grep -v 'Protocol error 1 on object' | head -20
jul 16 03:16:52 pop-os xdg-desktop-portal-cosmic[12987]: cosmic_corner_radius_layer_v1#167: error 1: CosmicCornerRadiusLayerV1 { id: ObjectId(cosmic_corner_radius_layer_v1@167[8], 254), version: 2, data: Some(Any { .. }), handle: WeakHandle { handle: WeakInnerHandle[rs] { .. } } } corner radius too large
jul 16 03:16:52 pop-os systemd[1845]: dbus-:1.1-org.freedesktop.impl.portal.desktop.cosmic@2.service: Main process exited, code=exited, status=1/FAILURE
jul 16 03:16:52 pop-os systemd[1845]: dbus-:1.1-org.freedesktop.impl.portal.desktop.cosmic@2.service: Failed with result 'exit-code'.
jul 16 03:16:52 pop-os systemd[1845]: dbus-:1.1-org.freedesktop.impl.portal.desktop.cosmic@2.service: Consumed 2.814s CPU time.
jul 16 03:17:27 pop-os cosmic-comp[1952]: Failed to force redraw for corner radius change.
jul 16 03:17:27 pop-os cosmic-comp[1952]: Failed to force redraw for corner radius reset.
jul 16 03:18:09 pop-os systemd[1845]: Started dbus-:1.1-org.freedesktop.impl.portal.desktop.cosmic@3.service.
jul 16 03:18:49 pop-os xdg-desktop-portal-cosmic[14151]: cosmic_corner_radius_layer_v1#281: error 1: CosmicCornerRadiusLayerV1 { id: ObjectId(cosmic_corner_radius_layer_v1@281[8], 2143), version: 2, data: Some(Any { .. }), handle: WeakHandle { handle: WeakInnerHandle[rs] { .. } } } corner radius too large
jul 16 03:18:50 pop-os systemd[1845]: dbus-:1.1-org.freedesktop.impl.portal.desktop.cosmic@3.service: Main process exited, code=dumped, status=11/SEGV
jul 16 03:18:50 pop-os systemd[1845]: dbus-:1.1-org.freedesktop.impl.portal.desktop.cosmic@3.service: Failed with result 'core-dump'.
jul 16 03:18:50 pop-os systemd[1845]: dbus-:1.1-org.freedesktop.impl.portal.desktop.cosmic@3.service: Consumed 3.345s CPU time.
jul 16 03:18:52 pop-os systemd[1845]: Started dbus-:1.1-org.freedesktop.impl.portal.desktop.cosmic@4.service.
jul 16 03:18:54 pop-os xdg-desktop-portal-cosmic[14277]: cosmic_corner_radius_layer_v1#167: error 1: CosmicCornerRadiusLayerV1 { id: ObjectId(cosmic_corner_radius_layer_v1@167[8], 251), version: 2, data: Some(Any { .. }), handle: WeakHandle { handle: WeakInnerHandle[rs] { .. } } } corner radius too large
jul 16 03:18:54 pop-os systemd[1845]: dbus-:1.1-org.freedesktop.impl.portal.desktop.cosmic@4.service: Main process exited, code=exited, status=1/FAILURE
jul 16 03:18:54 pop-os systemd[1845]: dbus-:1.1-org.freedesktop.impl.portal.desktop.cosmic@4.service: Failed with result 'exit-code'.
jul 16 03:18:56 pop-os systemd[1845]: Started dbus-:1.1-org.freedesktop.impl.portal.desktop.cosmic@5.service.

""""""""""""""""""""""""""""""""""""""""""""""""