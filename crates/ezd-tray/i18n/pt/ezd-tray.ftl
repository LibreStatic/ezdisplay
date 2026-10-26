## Linha de comando
about = ezdisplay: use seu tablet como segunda tela. Executa na bandeja do sistema.
arg-settings = Abre a janela de configurações.
arg-quit = Encerra o ezdisplay em execução e espera até que ele pare.
arg-autostart = Ativa ou desativa o início com a sessão e sai.
arg-elevation = Ativa ou desativa a execução como administrador e sai.
usage-label = Uso:
heading-options = Opções
help-flag = Mostra a ajuda
version-flag = Mostra a versão

## Estado do serviço
status-stopped = Parado
status-stopping = Parando…
status-waiting = Aguardando o tablet
status-unauthorized = Aceite a depuração USB no tablet
status-streaming = Transmitindo para { $device } · { $width }×{ $height } a { $hz } Hz
error-bind = Não foi possível escutar em { $addr }: { $reason }
error-adb-not-found = O adb não foi encontrado
error-adb-reverse = Falha no adb reverse para { $serial }: { $reason }
error-unsupported = Este sistema ainda não tem suporte a telas virtuais
error-source = Não foi possível criar a fonte de vídeo: { $reason }

## Menu da bandeja
menu-start = Iniciar
menu-stop = Parar
menu-scale = Escala
menu-scale-option = { $percent }%
menu-telemetry = Sobreposição de telemetria
menu-settings = Configurações…
menu-autostart = Iniciar com a sessão
menu-logs = Abrir a pasta de logs
menu-quit = Sair

## Janela: seções
section-status = Estado
section-display = Tela
section-input = Entrada
section-connection = Conexão
section-general = Geral
section-driver = Driver de vídeo

## Janela: estado
devices-heading = Dispositivos
stats-heading = Estatísticas ao vivo
stats-waiting = Aguardando as primeiras medições…
stat-device = Tablet
stat-mode = Modo
stat-encoder = Codificador
stat-fps = Taxa de quadros
stat-bitrate = Taxa de bits
stat-e2e = Latência da captura à tela
stat-decode = Tempo de decodificação
stat-transfer = Tempo de transferência
stat-input = Latência de entrada
stat-drops = Quadros descartados
stat-rtt = Ida e volta
mode-value = { $width }×{ $height } a { $hz } Hz, { $codec }
encoder-value = { $encoder } a { $mbps } Mbps
unit-fps = fps
unit-mbps = Mbps
value-hz = { $hz } Hz
value-ms = { $ms } ms
value-seconds = { $seconds } s
value-ms-pair = { $p50 } ms (percentil 95: { $p95 } ms)
value-drops = { $dropped } pelo tablet, { $late ->
        [one] { $late } confirmação atrasada
       *[other] { $late } confirmações atrasadas
    }

## Janela: tela
display-hint = As mudanças valem a partir da próxima conexão. Use "Aplicar agora" para reconectar já.
setting-scale = Escala
setting-refresh = Taxa de atualização máxima
setting-bitrate = Taxa de bits
bitrate-auto = Automática
setting-encoder = Codificador
encoder-auto = Automático
setting-gpu = Placa de vídeo
gpu-auto = Automática
gpu-prefer-dgpu = Preferir a placa de vídeo dedicada
gpu-named = Escolher pelo nome
gpu-name-hint = Nome do adaptador, como mostrado por "ezd-host diag"
apply-now = Aplicar agora (reconecta)
setting-hdr = HDR
setting-detach-grace = Remover após
hdr-auto = Automático
hdr-off = Desligado
hdr-hint-linux = O KDE ainda não envia HDR para telas virtuais; a transmissão continua em SDR.
stat-hdr = HDR
mode-value-hdr = { $width }×{ $height } a { $hz } Hz, { $codec } HDR10
hdr-reason-turned-off = O HDR está desligado nas configurações; convertendo para SDR
hdr-reason-old-client = O app do tablet é antigo demais para HDR; atualize-o
hdr-reason-display-not-hdr = A tela do tablet não suporta HDR10
hdr-reason-no-main10-decoder = O tablet não consegue decodificar vídeo HDR de 10 bits
hdr-reason-no-main10-encoder = Esta placa de vídeo não consegue codificar HEVC de 10 bits; convertendo para SDR

## Janela: entrada
input-enabled = Enviar a entrada por toque e caneta para este computador
setting-pen = A caneta funciona como
pen-tablet = Mesa digitalizadora
pen-mouse = Ponteiro de mouse
telemetry = Mostrar a sobreposição de telemetria no tablet
input-permission-denied = A entrada por toque e caneta precisa de permissão para criar dispositivos de entrada (/dev/uinput).
input-permission-missing = A entrada por toque e caneta precisa do módulo uinput do kernel, que não está carregado (falta /dev/uinput). Ao habilitar a entrada, ele é carregado e a permissão é configurada.
input-notice = A entrada por toque e caneta está bloqueada: o ezdisplay não tem permissão para criar dispositivos de entrada.
input-enable = Habilitar a entrada
input-enable-running = Aguardando a autorização…
input-enable-done = A entrada está habilitada.
input-enable-cancelled = Cancelado. Nada foi alterado.
input-enable-no-agent = Nenhum agente de autenticação está disponível. Instale a regra do udev manualmente, veja o README.
input-enable-still-denied = A regra está instalada, mas o acesso continua negado. Encerre a sessão, entre de novo e tente outra vez.
input-enable-failed = Não foi possível habilitar a entrada (código { $code }).
input-enable-error = Não foi possível executar a habilitação da entrada: { $reason }

## Janela: conexão
adb-sdk = { $path } (Android SDK)
adb-bundled = { $path } (incluído no ezdisplay)
devices-count =
    { $count ->
        [one] { $count } dispositivo
       *[other] { $count } dispositivos
    }
devices-none = Conecte o tablet com um cabo USB e ative a depuração USB.
samsung-driver = Driver USB Android da Samsung
device-line = { $name } ({ $serial }): { $state }
device-ready = pronto
device-unauthorized = aguardando você aceitar a depuração USB
device-offline = offline
open-app = Abrir o app no tablet ao conectar
use-adb = Conectar por USB com adb
advanced = Avançado
setting-port = Porta
port-hint = O tablet alcança este computador por esta porta. As mudanças valem quando não há sessão em andamento.

## Janela: geral
start-on-launch = Transmitir ao abrir o ezdisplay
setting-run-elevated = Controlar janelas de administrador (executar com privilégios elevados)
run-elevated-hint = Permite que a caneta e o toque alcancem janelas executadas como administrador, como o Gerenciador de Tarefas. Vale após reiniciar o ezdisplay.
about-version = ezdisplay { $version }
licenses = Licenças

## Janela: driver de vídeo
driver-state = Driver
driver-installed = Instalado
driver-missing = Não instalado
driver-owner = Instalado por
driver-owner-ours = ezdisplay
driver-owner-other = outro programa (não alterado)
driver-owner-none = —
driver-folder = Pasta do driver
driver-hint = Reparar reinstala o driver de tela virtual que acompanha o ezdisplay. O Windows pede permissão antes.
driver-repair = Reparar
repair-running = Reparando…
repair-cancelled = Cancelado. Nada foi alterado.
repair-done = O driver está instalado.
repair-restart = Reinicie o Windows para concluir a instalação do driver.
repair-failed = O reparo falhou (código { $code }).
repair-log = Detalhes: { $path }
repair-error = Não foi possível executar o reparo: { $reason }

## Window: cards and row descriptions
devices-empty = Nenhum dispositivo detectado
stats-empty = As estatísticas aparecem quando a transmissão começa
card-stream = Transmissão
card-measurements = Medições
card-picture = Qualidade de imagem
card-encoding = Codificação
card-virtual-monitor = Monitor virtual
card-adb = Android Debug Bridge
card-startup = Inicialização
card-appearance = Aparência
card-diagnostics = Diagnóstico
card-about = Sobre
card-driver = Estado do driver
card-maintenance = Manutenção
scale-description = Valores menores enviam menos pixels para o tablet.
refresh-description = Limita a taxa de atualização oferecida ao tablet.
bitrate-description = Qualidade do vídeo da transmissão. No automático, o ezdisplay escolhe para você.
encoder-description = O codificador de vídeo que comprime a transmissão.
hdr-description = Envia HDR10 quando a tela do tablet oferece suporte.
detach-grace-description = Quando o tablet entra em repouso, desliga ou se desconecta, o monitor virtual é removido após esse tempo e as janelas dele vão para seus outros monitores.
gpu-description = A placa de vídeo que captura e codifica a tela.
input-description = Use o tablet como tela de toque e caneta neste computador.
pen-description = Uma mesa digitalizadora adiciona pressão e inclinação.
telemetry-description = Mostra números de desempenho ao vivo no tablet durante a transmissão.
use-adb-description = Encontra tablets com a depuração USB ativada.
autostart-description = Inicia o ezdisplay quando você entra na sessão.
start-on-launch-description = Começa a esperar pelo tablet assim que o ezdisplay abre.
system-accent = Usar a cor de destaque do sistema
system-accent-description = Usa a cor de destaque que você escolheu nas configurações do sistema. Desligado, o ezdisplay usa a própria cor.
logs-title = Logs
logs-description = Abre a pasta onde o ezdisplay grava seus logs.
about-description = Tela USB de código aberto para tablets Android.
repository = Código-fonte
driver-reinstall = Reinstalar o driver

## Window: Wi-Fi
card-wifi = Wi-Fi
wifi-enable = Conectar por Wi-Fi
wifi-latency-warning = O Wi-Fi acrescenta latência e jitter; o USB é o melhor para desenhar.
wifi-starts-with-stream = O Wi-Fi é ativado quando a transmissão começa.
wifi-address = Endereço
wifi-fingerprint = Impressão digital do host
wifi-pair = Parear um dispositivo
wifi-pair-cancel = Cancelar
wifi-pair-hint = No tablet, escaneie este QR code ou digite o PIN.
wifi-pin = PIN
wifi-qr-label = QR code de pareamento
wifi-pair-expires = Fecha em { $seconds } s
wifi-notice-paired = Pareado com { $device }.
wifi-notice-attempts = O pareamento foi fechado: PINs incorretos demais.
wifi-notice-expired = O pareamento foi fechado: o tempo acabou.
wifi-paired-count =
    { $count ->
        [one] { $count } dispositivo pareado
       *[other] { $count } dispositivos pareados
    }
wifi-paired-none = Ainda não há dispositivos pareados.
wifi-forget = Esquecer
error-wifi = Não foi possível iniciar o Wi-Fi: { $reason }
