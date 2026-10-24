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

## Entrada (lapiz y tacto)

El host inyecta el lapiz y el tacto de la tablet con dispositivos de puntero sinteticos de
Windows (Windows 10 1809 o posterior), y los clics, el scroll y las teclas con `SendInput`.

1. **Diagnostico.** En `diag.json`, la seccion `input` debe decir `"pen": "ok"` y `"touch": "ok"`.
   Si no, copiar el error (HRESULT) tal cual.
2. **Sin tablet.** Con `.\ezd-host.exe run --source virtual` en una consola, ejecutar en otra
   `.\ezd-fake-client.exe` con `--input-script drag`, `touch2` y `scroll` (uno por vez; ver
   `.\ezd-fake-client.exe --help` por las demas opciones) y mirar el cursor y la tinta en Paint o
   en Pizarra (Whiteboard) en el monitor virtual. `drag` dibuja un trazo, `touch2` hace dos dedos,
   `scroll` rueda y zoom con Ctrl, `keys` mantiene Ctrl+Espacio mientras dibuja un trazo.
3. **Presion.** Con la tablet y un lapiz: en Paint o Pizarra el grosor del trazo debe seguir la
   presion; el borrador (si el lapiz tiene) debe borrar.
4. **Tacto.** En Edge: arrastrar con un dedo desplaza la pagina, pellizcar con dos dedos hace zoom.
   Un dedo quieto no debe soltarse solo (el host lo refresca cada 100 ms; si se suelta, anotarlo).
5. **Boton del lapiz.** Boton lateral apretado + toque = clic derecho (menu contextual).
6. **Atajos de teclado (perfil Tableta gráfica).** En la app de la tablet elegir el perfil
   "Tableta gráfica". En Krita (Settings > Configure Krita > Tablet settings) elegir Windows Ink
   para que haya presion. Mantener apretado el boton del lapiz y arrastrar
   con la punta debe hacer zoom (atajo por defecto: Ctrl+Espacio). Sin tablet,
   `.\ezd-fake-client.exe --input-script keys` reproduce lo mismo. En la pagina de prueba
   (`tools/input-test`) las teclas aparecen como `keydown` / `keyup`, con Ctrl mantenido durante
   el trazo. Anotar si alguna tecla queda "pegada" al soltar el boton o al cortar la sesion.
7. **Ventanas elevadas.** Con el Administrador de tareas abierto (elevado): Windows bloquea la
   entrada hacia el, salvo que el host tambien corra como administrador. Anotar si en el log de
   `ezd-host run` aparece el aviso de UIPI y si el lapiz/tacto no tienen efecto ahi.
8. Anotar el campo `note` del estado de entrada que muestra la tablet y cualquier linea
   `input:` del log (con `RUST_LOG=debug`).

## Que enviar de vuelta

- `diag.json` y `vdd-status.json` de cada modo MUX (indicar cual era).
- La salida de consola (stderr) de `ezd-host run ...` desde que arranca hasta el problema;
  si algo falla, repetir con `RUST_LOG=debug`.
- Una descripcion de lo observado: ¿se creo el monitor?, ¿llego video?, ¿cuanta latencia
  aproximada?, ¿el puntero iba bien?, ¿hubo parpadeos o pantalla negra?
- Modelo de laptop, GPU(s) y version de driver (aparecen en `diag.json`).
