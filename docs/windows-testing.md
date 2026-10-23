# Prueba del host de Windows

Esta guia sirve para probar el host de Windows (Fase 1d) en una maquina real y devolver
la informacion necesaria para corregir lo que falle. El host crea un monitor virtual con
el Virtual Display Driver (VDD), lo captura con Desktop Duplication y lo codifica en la GPU.

Hay que probarlo en las dos configuraciones de la laptop (la pantalla interna cuelga de
la iGPU o de la dGPU segun el modo MUX) y, si se puede, con una tercera pasada forzando la GPU.

## Requisitos

- Windows 10 21H2 o Windows 11, con los drivers de GPU al dia.
- Una tablet Android con depuracion USB activada y `adb` en el PATH.
- Permisos de administrador una sola vez (paso 5).

## Pasos

1. **Descargar el artefacto de CI.** En GitHub, pestana *Actions*, ultima ejecucion verde,
   artefacto `ezdisplay-windows-x86_64`. Descomprimirlo en una carpeta, por ejemplo
   `C:\ezdisplay`. Contiene `ezd-host.exe`, `ezd-fake-client.exe` y las DLL de FFmpeg
   (deben quedar junto al `.exe`).

2. **Instalar el VDD** (una vez):

   ```powershell
   winget install --id=VirtualDrivers.Virtual-Display-Driver -e
   ```

   Aceptar la instalacion del driver cuando Windows lo pida.

3. **Diagnostico completo.** Desde la carpeta del artefacto:

   ```powershell
   .\ezd-host.exe diag > diag.json
   ```

   No cambia nada en el sistema. Incluye version de Windows y DPI, adaptadores (LUID,
   VRAM, orden de rendimiento/consumo), salidas por adaptador, la decision de GPU,
   el estado del VDD, una prueba de codificador por adaptador (1920x1080, un cuadro) y una
   prueba de Desktop Duplication sobre el monitor virtual.

4. **Estado del VDD:**

   ```powershell
   .\ezd-host.exe vdd status > vdd-status.json
   ```

5. **Preparar el VDD** (una vez, en una consola como administrador):

   ```powershell
   .\ezd-host.exe vdd setup
   ```

   Da permiso de escritura solo sobre `vdd_settings.xml`, en la carpeta de configuracion del VDD
   (`C:\VirtualDisplayDriver` o el valor `VDDPATH` del registro), para que las siguientes
   ejecuciones no necesiten administrador. Opciones utiles: `--gpu "NOMBRE"` para asignar
   el monitor virtual a una GPU, y `--mode 2560x1600@120` (repetible) para ofrecer modos
   por adelantado. `.\ezd-host.exe vdd revert` deshace el permiso.

6. **Instalar la app en la tablet** (una vez por version). Del workflow *Android* en
   *Actions*, artefacto `ezdisplay-android-apks`:

   ```powershell
   adb install -r app-full-debug.apk
   ```

7. **Transmitir** (sin administrador, con la tablet conectada por USB):

   ```powershell
   .\ezd-host.exe run --source virtual --adb
   ```

   Abrir la app en la tablet. Para elegir GPU a mano: `--adapter "NVIDIA"` (nombre o LUID
   `0x...`) o `--prefer-dgpu`. Para capturar un monitor que ya existe: `--output \\.\DISPLAY3`.
   Con `$env:RUST_LOG="debug"` el log es mas detallado.

## Que probar en cada modo MUX

Repetir los pasos 3, 4 y 7 una vez por modo:

- **Modo hibrido / Optimus (la pantalla interna usa la iGPU):** la politica elige la iGPU
  (la que maneja la pantalla principal). Esperado: codificador `h264_qsv` (Intel) y
  `adapter_mismatch: false` en `vdd_output`.
- **Modo dGPU / MUX discreto (la pantalla interna usa la dGPU):** la politica elige la dGPU.
  Esperado: `h264_nvenc` (NVIDIA) o `h264_amf` (AMD).
- **Forzando la dGPU en modo hibrido:** `--prefer-dgpu`. Esperado: el host asigna el VDD a la
  dGPU (SETGPU), y el video sigue fluyendo.

En cada caso comprobar: que el monitor virtual aparece a la derecha del principal, que la
imagen llega a la tablet con el modo pedido, que el puntero se mueve sin retraso, y que
al cambiar la resolucion o bloquear/desbloquear la sesion el video se recupera solo.

## Que enviar de vuelta

- `diag.json` y `vdd-status.json` de cada modo MUX (indicar cual era).
- La salida de consola (stderr) de `ezd-host run ...` desde que arranca hasta el problema;
  si algo falla, repetir con `RUST_LOG=debug`.
- Una descripcion de lo observado: ¿se creo el monitor?, ¿llego video?, ¿cuanta latencia
  aproximada?, ¿el puntero iba bien?, ¿hubo parpadeos o pantalla negra?
- Modelo de laptop, GPU(s) y version de driver (aparecen en `diag.json`).
