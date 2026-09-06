#!/usr/bin/env python3
"""Reidentifica las pantallas y corrige profiles.kdl contra el hardware real.

Dos cosas que se rompen solas con el tiempo (cambio de puerto, update de
driver, cambio de panel):

  1. El `connector=` de un `monitor` deja de apuntar al panel correcto.
  2. Un `refresh=` de un `output` deja de existir en la lista de modos, o
     reaparece mas tarde (adaptadores que enganchan 75 Hz solo a veces).

Para el caso 2 la correccion es bidireccional. Cuando el refresco declarado no
esta disponible se baja al mas cercano y el deseado queda anotado en un atributo
`refresh-want`; cuando ese refresco vuelve a aparecer en la lista de modos, la
siguiente corrida lo restaura sola y borra el marcador. Asi el archivo nunca
pierde cual era la intencion original.

El script lee `cosmic-randr list --kdl`, reidentifica cada monitor por su
identidad EDID (make/model/serial) en vez de por el nombre del conector, y
reescribe en el sitio los `connector=` y `refresh=` que quedaron mal. Los
comentarios y el formato del archivo se preservan: solo se tocan los valores.

La identidad esperada de cada monitor se declara en el mismo profiles.kdl, con
atributos `match-make` / `match-model` / `match-serial` en el nodo `monitor`.
Si faltan, el script los infiere del hardware que hoy responde en ese conector
y los deja escritos, de modo que la proxima corrida ya pueda reidentificar
aunque el conector haya cambiado.

  ./retune-profiles.py              # diagnostica y muestra el diff (no escribe)
  ./retune-profiles.py --write      # aplica los cambios (guarda un .bak)
  ./retune-profiles.py --file X.kdl # sobre otro archivo
"""

import argparse
import difflib
import os
import re
import shutil
import subprocess
import sys

DEFAULT_PATH = os.path.join(
    os.environ.get("XDG_CONFIG_HOME") or os.path.expanduser("~/.config"),
    "screen-switcher",
    "profiles.kdl",
)

# Un serial solo sirve para identificar si es realmente unico. Varios EDID
# traen literales de relleno que repiten entre paneles distintos.
SERIAL_PLACEHOLDERS = {"", "serialnumber", "0x00000000", "none", "unknown", "n/a"}


# -- Lectura del hardware ----------------------------------------------------

class Output:
    def __init__(self, name):
        self.name = name
        self.make = ""
        self.model = ""
        self.serial = ""
        self.modes = []  # (w, h, mhz)

    @property
    def identity(self):
        return f"{self.make}/{self.model}"

    def has_serial(self):
        return self.serial.strip().lower() not in SERIAL_PLACEHOLDERS

    def refreshes_for(self, w, h):
        """Refrescos disponibles (Hz, float) para esa resolucion, desc."""
        return sorted({m[2] / 1000.0 for m in self.modes if m[0] == w and m[1] == h},
                      reverse=True)


def read_hardware():
    """Parsea `cosmic-randr list --kdl`. Es KDL plano y muy regular, asi que
    alcanza con un scan por lineas y no hace falta una dependencia extra."""
    try:
        raw = subprocess.run(["cosmic-randr", "list", "--kdl"],
                             capture_output=True, text=True, check=True).stdout
    except FileNotFoundError:
        sys.exit("error: no encuentro `cosmic-randr` en el PATH")
    except subprocess.CalledProcessError as e:
        sys.exit(f"error: `cosmic-randr list --kdl` fallo: {e.stderr.strip()}")

    outputs, cur, in_modes = [], None, False
    for line in raw.splitlines():
        s = line.strip()
        m = re.match(r'^output\s+"([^"]+)"', s)
        if m:
            cur = Output(m.group(1))
            outputs.append(cur)
            in_modes = False
            continue
        if cur is None:
            continue
        if s.startswith("description"):
            cur.make = (re.search(r'make="([^"]*)"', s) or [None, ""])[1]
            cur.model = (re.search(r'model="([^"]*)"', s) or [None, ""])[1]
        elif s.startswith("serial_number"):
            cur.serial = (re.search(r'"([^"]*)"', s) or [None, ""])[1]
        elif s.startswith("modes"):
            in_modes = True
        elif s == "}":
            in_modes = False
        elif in_modes and s.startswith("mode "):
            n = re.findall(r"\d+", s.split("current")[0].split("preferred")[0])
            if len(n) >= 3:
                cur.modes.append((int(n[0]), int(n[1]), int(n[2])))
    return outputs


# -- Lectura de los perfiles -------------------------------------------------

def attr(line, key):
    m = re.search(rf'\b{re.escape(key)}="([^"]*)"', line)
    return m.group(1) if m else None


def set_attr(line, key, value):
    """Reemplaza key="..." si existe; si no, lo agrega al final del nodo."""
    pat = rf'(\b{re.escape(key)}=")([^"]*)(")'
    if re.search(pat, line):
        return re.sub(pat, lambda m: m.group(1) + value + m.group(3), line)
    body = line.rstrip("\n").rstrip()
    pad = line[len(line.rstrip("\n")):]
    return f'{body} {key}="{value}"{pad}'


def parse_profiles(lines):
    """Devuelve (monitors, outputs) con indices de linea para poder editar.

    monitors: key -> {line, connector, match_make, match_model, match_serial}
    outputs:  lista de {line, profile, monitor_key, w, h, refresh}
    """
    monitors, outputs, profile = {}, [], None
    for i, line in enumerate(lines):
        s = line.strip()
        if s.startswith("//"):
            continue
        m = re.match(r'^monitor\s+"([^"]+)"', s)
        if m:
            monitors[m.group(1)] = {
                "line": i,
                "connector": attr(line, "connector"),
                "match_make": attr(line, "match-make"),
                "match_model": attr(line, "match-model"),
                "match_serial": attr(line, "match-serial"),
            }
            continue
        m = re.match(r'^profile\s+"([^"]+)"', s)
        if m:
            profile = m.group(1)
            continue
        m = re.match(r'^output\s+"([^"]+)"', s)
        if m:
            w, h = attr_num(line, "width"), attr_num(line, "height")
            outputs.append({
                "line": i,
                "profile": profile,
                "monitor_key": m.group(1),
                "w": int(w) if w is not None else 1920,
                "h": int(h) if h is not None else 1080,
                "refresh": attr_num(line, "refresh"),
                "refresh_want": attr_num(line, "refresh-want"),
            })
    return monitors, outputs


def attr_num(line, key):
    m = re.search(rf"\b{re.escape(key)}=(-?[\d.]+)", line)
    return float(m.group(1)) if m else None


def set_num(line, key, value, after=None):
    """Reemplaza key=N si existe. Si no, lo inserta detras del atributo `after`
    (para que `refresh-want` quede pegado a `refresh` y se lea de corrido), o
    al final del nodo como ultimo recurso."""
    txt = f"{value:g}"
    pat = rf"(\b{re.escape(key)}=)(-?[\d.]+)"
    if re.search(pat, line):
        return re.sub(pat, lambda m: m.group(1) + txt, line)
    if after:
        apat = rf"(\b{re.escape(after)}=-?[\d.]+)"
        if re.search(apat, line):
            return re.sub(apat, lambda m: f"{m.group(1)} {key}={txt}", line, count=1)
    body = line.rstrip("\n").rstrip()
    pad = line[len(line.rstrip("\n")):]
    return f"{body} {key}={txt}{pad}"


def del_num(line, key):
    return re.sub(rf"\s*\b{re.escape(key)}=-?[\d.]+", "", line)


# -- Reidentificacion --------------------------------------------------------

def resolve(mon, key, hardware, notes):
    """Elige el conector que hoy corresponde a este monitor.

    Prioridad: serial unico > make+model > el conector que ya tenia. El serial
    manda porque sobrevive a un cambio de puerto Y distingue dos paneles del
    mismo modelo; make+model es el fallback util cuando el EDID no trae un
    serial de verdad (que es lo habitual)."""
    want_serial = (mon["match_serial"] or "").strip().lower()
    if want_serial and want_serial not in SERIAL_PLACEHOLDERS:
        for o in hardware:
            if o.has_serial() and o.serial.strip().lower() == want_serial:
                return o, "serial"

    want_make, want_model = mon["match_make"], mon["match_model"]
    if want_make or want_model:
        hits = [o for o in hardware
                if (want_make is None or o.make == want_make)
                and (want_model is None or o.model == want_model)]
        if len(hits) == 1:
            return hits[0], "make+model"
        if len(hits) > 1:
            # Ambiguo: varios paneles identicos y sin serial que los separe.
            # Preferimos no adivinar y quedarnos con el conector declarado.
            same = [o for o in hits if o.name == mon["connector"]]
            if same:
                notes.append(f"  ! '{key}': {len(hits)} pantallas coinciden con "
                             f"{want_make}/{want_model}; me quedo con "
                             f"{mon['connector']} (declara match-serial para "
                             f"desambiguar)")
                return same[0], "ambiguo"
            notes.append(f"  ! '{key}': {len(hits)} pantallas coinciden con "
                         f"{want_make}/{want_model} y ninguna esta en "
                         f"{mon['connector']}; no toco nada")
            return None, "ambiguo"

    for o in hardware:
        if o.name == mon["connector"]:
            return o, "conector"
    return None, "ausente"


def resolve_refresh(out, w, h, declared, wanted):
    """Que par (refresh, refresh-want) deberia tener esta salida.

    `declared` es el `refresh=` que hay hoy en el archivo. `wanted` es el
    `refresh-want=`: el refresco que el usuario quiere de verdad, anotado la vez
    que hubo que bajarlo porque no estaba en la lista de modos.

    La correccion va en los dos sentidos. Si el deseado no esta disponible, se
    baja al mas cercano y se deja anotado el deseado. Si el deseado volvio a
    aparecer -- el caso del adaptador que a veces engancha 75 Hz y a veces no --
    se restaura y se borra el marcador, que ya no hace falta.

    Devuelve (refresh, want) con want=None si no corresponde marcador, o None si
    la resolucion entera no existe en esta pantalla."""
    target = wanted if wanted is not None else declared
    if target is None:
        return None  # sin refresh declarado: cosmic-randr elige el preferido
    avail = out.refreshes_for(w, h)
    if not avail:
        return None
    # Tolerancia +-0.5 Hz, la misma que usa cosmic-randr al matchear el modo.
    if any(abs(hz - target) <= 0.5 for hz in avail):
        return (target, None)
    best = min(avail, key=lambda hz: (abs(hz - target), -hz))
    return (best, target)


# -- Main --------------------------------------------------------------------

def main():
    ap = argparse.ArgumentParser(
        description="Reidentifica pantallas y corrige profiles.kdl.")
    ap.add_argument("--file", default=DEFAULT_PATH,
                    help=f"profiles.kdl a corregir (default: {DEFAULT_PATH})")
    ap.add_argument("--write", action="store_true",
                    help="escribir los cambios (por defecto solo muestra el diff)")
    args = ap.parse_args()

    if not os.path.exists(args.file):
        sys.exit(f"error: no existe {args.file}")

    original = open(args.file, encoding="utf-8").read()
    lines = original.splitlines(keepends=True)
    hardware = read_hardware()
    monitors, outputs = parse_profiles(lines)

    print("Pantallas detectadas:")
    for o in hardware:
        ser = o.serial if o.has_serial() else "(sin serial util)"
        print(f"  {o.name:<10} {o.identity:<40} {ser}")
    print()

    notes, mapping = [], {}

    # 1. Reidentificar cada monitor y refrescar su connector / match-*.
    for key, mon in monitors.items():
        out, how = resolve(mon, key, hardware, notes)
        mapping[key] = out
        if out is None:
            notes.append(f"  ! '{key}': no encuentro la pantalla "
                         f"(connector={mon['connector']}); la dejo como esta")
            continue
        i = mon["line"]
        if out.name != mon["connector"]:
            notes.append(f"  * '{key}': {mon['connector']} -> {out.name} "
                         f"(por {how}: {out.identity})")
            lines[i] = set_attr(lines[i], "connector", out.name)
        # Anclar la identidad para la proxima corrida.
        if mon["match_make"] != out.make:
            lines[i] = set_attr(lines[i], "match-make", out.make)
        if mon["match_model"] != out.model:
            lines[i] = set_attr(lines[i], "match-model", out.model)
        if out.has_serial() and mon["match_serial"] != out.serial:
            lines[i] = set_attr(lines[i], "match-serial", out.serial)

    # 2. Validar la geometria de cada output contra los modos reales.
    for spec in outputs:
        out = mapping.get(spec["monitor_key"])
        if out is None:
            continue
        label = f"perfil '{spec['profile']}' / {spec['monitor_key']}"
        if not out.refreshes_for(spec["w"], spec["h"]):
            notes.append(f"  ! {label}: {out.name} no soporta "
                         f"{spec['w']}x{spec['h']}; hay que elegir otra "
                         f"resolucion a mano")
            continue
        if spec["refresh"] is None:
            continue
        res = resolve_refresh(out, spec["w"], spec["h"],
                              spec["refresh"], spec["refresh_want"])
        if res is None:
            continue
        new_refresh, new_want = res
        if new_refresh == spec["refresh"] and new_want == spec["refresh_want"]:
            continue

        avail = ", ".join(f"{hz:g}" for hz in out.refreshes_for(spec["w"], spec["h"]))
        if new_want is None:
            notes.append(f"  ^ {label}: {new_refresh:g} Hz volvio a estar "
                         f"disponible -> lo recupero (estaba en "
                         f"{spec['refresh']:g} Hz)")
        else:
            notes.append(f"  ~ {label}: {new_want:g} Hz no esta disponible -> "
                         f"{new_refresh:g} Hz (hay: {avail}). Queda anotado en "
                         f"refresh-want para restaurarlo cuando vuelva")

        line = set_num(lines[spec["line"]], "refresh", new_refresh)
        line = (del_num(line, "refresh-want") if new_want is None
                else set_num(line, "refresh-want", new_want, after="refresh"))
        lines[spec["line"]] = line

    if notes:
        print("Hallazgos:")
        print("\n".join(notes))
        print()

    updated = "".join(lines)
    if updated == original:
        print("Sin cambios: los perfiles ya coinciden con el hardware.")
        return 0

    diff = difflib.unified_diff(original.splitlines(True), updated.splitlines(True),
                                fromfile=args.file, tofile=args.file + " (corregido)")
    print("".join(diff))

    if not args.write:
        print("\n(dry-run) volve a correr con --write para aplicar.")
        return 0

    shutil.copy2(args.file, args.file + ".bak")
    with open(args.file, "w", encoding="utf-8") as f:
        f.write(updated)
    print(f"\nEscrito {args.file} (backup en {args.file}.bak)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
