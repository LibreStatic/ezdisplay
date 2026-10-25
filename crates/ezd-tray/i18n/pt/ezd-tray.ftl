## Linha de comando
about = ezdisplay: use seu tablet como segunda tela. Executa na bandeja do sistema.
arg-settings = Abre a janela de configurações.
arg-quit = Encerra o ezdisplay em execução e espera até que ele pare.
arg-autostart = Ativa ou desativa o início com a sessão e sai.
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
value-fps = { $fps } fps
value-mbps = { $mbps } Mbps
value-hz = { $hz } Hz
value-ms = { $ms } ms
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

## Janela: entrada
input-enabled = Enviar a entrada por toque e caneta para este computador
setting-pen = A caneta funciona como
pen-tablet = Mesa digitalizadora (pressão e inclinação)
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
devices-none = Nenhum dispositivo listado. Conecte o tablet com um cabo USB e ative a depuração USB.
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
