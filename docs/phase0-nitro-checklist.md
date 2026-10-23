# Fase 0 — checklist en la Acer Nitro 5 (Windows)

Objetivo: confirmar con datos por qué la app de segunda pantalla actual se traba (en qué GPU queda su pantalla y si hay copias entre GPUs) y dejar una línea base de latencia para comparar ezdisplay. Después, probar si Virtual Display Driver (VDD) queda en la GPU que elegimos.

Hacé las partes A y B **en los dos modos del MUX** (Discreta y Optimus/Híbrido; el cambio se hace en NitroSense y pide reiniciar). Completá la tabla del final y pegámela en el chat junto con las capturas.

## 0. Datos de base (una sola vez)

- Windows: `winver` → versión y build.
- Modelo exacto de la Nitro (etiqueta de abajo o `wmic csproduct get name`).
- Administrador de dispositivos → Adaptadores de pantalla: nombre **exacto** de las dos GPU y versión de cada driver.
- Monitor principal: ¿el panel de la notebook o uno externo? Si es externo, ¿por HDMI, USB-C o Mini DP?
- Tablet: modelo exacto, versión de Android y resolución nativa (Ajustes → Acerca de la tablet, o `adb shell wm size`).
- App de segunda pantalla actual: versión y configuración (resolución, fps, calidad, USB/Wi-Fi). Cable usado.

## A. App de segunda pantalla actual (línea base)

1. **¿A qué GPU está conectada la pantalla de la app actual?** Configuración → Sistema → Pantalla → Pantalla avanzada → elegí la pantalla de la app actual → anotá "Conectada a: …". Repetí con el monitor principal.
2. **Motores de cada GPU.** Administrador de tareas → Rendimiento → GPU 0 y GPU 1. En cada una cambiá dos de los gráficos chicos (clic en el título del gráfico) a **Copy** y **Video Encode**. Anotá el % aproximado de cada motor en cada GPU en estos casos:
   - a. Quieto, con la tablet mostrando el escritorio.
   - b. Moviendo el cursor de un lado a otro entre el monitor principal y la tablet.
   - c. Arrastrando una ventana (por ejemplo, el navegador) entre las dos pantallas.
   - d. Un video de YouTube reproduciéndose en la tablet.

   Sacá una captura del Administrador de tareas en el caso c.
3. **Qué GPU usa el proceso.** Administrador de tareas → Detalles → clic derecho en los encabezados → Seleccionar columnas → "Motor de GPU". Anotá lo que muestra para los procesos de la app actual.
4. **Latencia.** Abrí `tools/latency/index.html` del repo en **dos** ventanas del navegador: una maximizada en el monitor principal y otra maximizada en la tablet. En la ventana de la tablet, la línea de arriba a la izquierda muestra a qué refresco llega realmente (anotalo).
   - Con el celular en cámara lenta (240 fps si se puede), filmá las dos pantallas juntas unos 10 segundos.
   - Después filmá el cursor cruzando de una pantalla a la otra, y una ventana arrastrándose entre las dos.
   - Mandame los videos, o medí vos: pausá en un cuadro y restá los dos relojes (tablet − principal). Tomá unas 10 lecturas en cuadros distintos.

## B. Virtual Display Driver (prueba de concepto)

> Instala un driver de pantalla firmado (MIT). Se desinstala desde "Agregar o quitar programas" o con `winget uninstall`.

1. En una terminal como administrador: `winget install --id=VirtualDrivers.Virtual-Display-Driver -e`
2. Si existe la clave `HKLM\SOFTWARE\MikeTheTech\VirtualDisplayDriver` en regedit, anotá qué valores tiene: el driver la lee **antes** que el XML.
3. Editá `C:\VirtualDisplayDriver\vdd_settings.xml`:
   - En `<gpu><friendlyname>` poné el nombre exacto de la GPU a probar, copiado del Administrador de dispositivos. Primero la NVIDIA y después la Intel/AMD.
   - En `<resolutions>` agregá la resolución nativa de la tablet. Los refrescos de `<global>` (60–240) se suman a todas las resoluciones.

     ```xml
     <resolution>
         <width>2560</width>
         <height>1600</height>
         <refresh_rate>120</refresh_rate>
     </resolution>
     ```
4. Reiniciá el driver: Administrador de dispositivos → Adaptadores de pantalla → Virtual Display Driver → Deshabilitar y Habilitar.
5. Configuración → Pantalla: poné el monitor virtual en **Extender**, con la resolución de la tablet y **120 Hz**. En Pantalla avanzada, anotá a qué GPU dice "Conectada a".
6. Repetí el paso A.2 caso c, arrastrando una ventana al monitor virtual. No se ve, pero se puede mover con `Win+Shift+←/→`. Anotá el % de Copy de cada GPU.
7. Cambiá `friendlyname` a la otra GPU, reiniciá el driver (paso 4) y repetí 5–6.

## Tabla para completar

| Modo MUX | Prueba | Pantalla conectada a | Copy GPU0 / GPU1 (%) | Video Encode GPU0 / GPU1 (%) | Latencia tablet − principal (ms) | Refresco real en la tablet |
|---|---|---|---|---|---|---|
| Discreta | App actual quieto | | | | | |
| Discreta | App actual cursor cruzando | | | | | |
| Discreta | App actual arrastrando ventana | | | | | |
| Discreta | App actual video | | | | | |
| Discreta | VDD → NVIDIA, arrastrando | | | | — | — |
| Discreta | VDD → iGPU, arrastrando | | | | — | — |
| Híbrido | App actual quieto | | | | | |
| Híbrido | App actual cursor cruzando | | | | | |
| Híbrido | App actual arrastrando ventana | | | | | |
| Híbrido | App actual video | | | | | |
| Híbrido | VDD → NVIDIA, arrastrando | | | | — | — |
| Híbrido | VDD → iGPU, arrastrando | | | | — | — |

Notas libres: lo que se sienta distinto, errores del driver, si alguna resolución o refresco no aparece, etc.
