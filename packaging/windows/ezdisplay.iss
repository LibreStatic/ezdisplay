; ezdisplay Windows installer (Inno Setup 6.7).
;
; Build (see README.md):
;   ISCC.exe /DAppVersion=0.1.0 /DStagingDir=C:\path\to\staging ezdisplay.iss
; The staging folder is produced by stage.ps1.

#include "pins.iss"

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef StagingDir
  #define StagingDir "staging"
#endif

[Setup]
AppId={{667B0CAF-83D5-435D-A4AA-F437C5587467}
AppName=ezdisplay
AppVersion={#AppVersion}
AppPublisher=LibreStatic
AppPublisherURL=https://github.com/LibreStatic/ezdisplay
AppSupportURL=https://github.com/LibreStatic/ezdisplay/issues
DefaultDirName={autopf}\ezdisplay
DisableProgramGroupPage=yes
DisableDirPage=auto
UsePreviousAppDir=yes
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Floor of the BtbN FFmpeg builds.
MinVersion=10.0.19045
WizardStyle=modern
SetupIconFile=..\..\crates\ezd-tray\assets\ezdisplay.ico
UninstallDisplayIcon={app}\ezdisplay.exe
CloseApplications=yes
RestartApplications=no
; Required to extract the FFmpeg .zip (without it ExtractArchive only handles .7z).
ArchiveExtraction=full
OutputDir=output
OutputBaseFilename=ezdisplay-setup-{#AppVersion}-x64
Compression=lzma2
SolidCompression=yes
; No code-signing certificate yet: SmartScreen shows "Run anyway" and Smart App Control blocks
; unsigned installers. Enable once a SignPath Foundation certificate exists (define the tool
; with the SignTool directive or the ISCC /S option).
;SignTool=signpath

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "spanish"; MessagesFile: "compiler:Languages\Spanish.isl,i18n\es-voseo.isl"
Name: "french"; MessagesFile: "compiler:Languages\French.isl"
Name: "brazilianportuguese"; MessagesFile: "compiler:Languages\BrazilianPortuguese.isl"
Name: "italian"; MessagesFile: "compiler:Languages\Italian.isl,i18n\it-tu.isl"
Name: "german"; MessagesFile: "compiler:Languages\German.isl,i18n\de-du.isl"

[CustomMessages]
english.TaskDriver=Install the virtual display driver
english.TaskAutostart=Start ezdisplay when you sign in
english.TaskUsbDk=USB without developer mode (installs the UsbDk driver)
english.DownloadingUsbDk=Downloading UsbDk
english.UsbDkDownloadFailed=UsbDk could not be downloaded:%n%n%1%n%nClick Retry to try again or Cancel to exit Setup. You can also go back and clear the USB task.
english.UsbDkInstallFailed=UsbDk could not be installed (code %1). USB without developer mode will not work; ezdisplay falls back to adb.
english.InstallingUsbDk=Installing the UsbDk driver...
english.DownloadTitle=Downloading components
english.DownloadDesc=Setup is downloading the components it needs: the FFmpeg video libraries (about 90 MB) and, if selected, UsbDk.
english.ExtractingFfmpeg=Extracting FFmpeg...
english.InstallingDriver=Installing the virtual display driver...
english.FfmpegDownloadFailed=FFmpeg could not be downloaded:%n%n%1%n%nClick Retry to try again or Cancel to exit Setup.
english.FfmpegZipBadHash=The file given with /FFMPEGZIP does not match the expected SHA-256 checksum.
english.FfmpegInstallFailed=FFmpeg could not be installed: %1
english.DriverFailed=The virtual display driver could not be installed (code %1).%n%nDetails: %2%n%nDo you want to try again? Choose No to skip it; you can repair it later from ezdisplay's settings (Display driver, Repair).

spanish.TaskDriver=Instalar el controlador de pantalla virtual
spanish.TaskAutostart=Iniciar ezdisplay al iniciar sesión
spanish.TaskUsbDk=USB sin modo desarrollador (instala el controlador UsbDk)
spanish.DownloadingUsbDk=Descargando UsbDk
spanish.UsbDkDownloadFailed=No se pudo descargar UsbDk:%n%n%1%n%nHacé clic en Reintentar para probar de nuevo o en Cancelar para salir. También podés volver y desmarcar la tarea de USB.
spanish.UsbDkInstallFailed=No se pudo instalar UsbDk (código %1). El USB sin modo desarrollador no va a funcionar; ezdisplay usa adb como alternativa.
spanish.InstallingUsbDk=Instalando el controlador UsbDk...
spanish.DownloadTitle=Descargando componentes
spanish.DownloadDesc=El asistente está descargando los componentes que necesita: las bibliotecas de video FFmpeg (unos 90 MB) y, si lo elegiste, UsbDk.
spanish.ExtractingFfmpeg=Extrayendo FFmpeg...
spanish.InstallingDriver=Instalando el controlador de pantalla virtual...
spanish.FfmpegDownloadFailed=No se pudo descargar FFmpeg:%n%n%1%n%nHacé clic en Reintentar para probar de nuevo o en Cancelar para salir.
spanish.FfmpegZipBadHash=El archivo indicado con /FFMPEGZIP no coincide con la suma SHA-256 esperada.
spanish.FfmpegInstallFailed=No se pudo instalar FFmpeg: %1
spanish.DriverFailed=No se pudo instalar el controlador de pantalla virtual (código %1).%n%nDetalles: %2%n%n¿Querés intentarlo de nuevo? Elegí No para omitirlo; podés repararlo después desde los ajustes de ezdisplay (Controlador de pantalla, Reparar).

french.TaskDriver=Installer le pilote d'affichage virtuel
french.TaskAutostart=Lancer ezdisplay à l'ouverture de session
french.TaskUsbDk=USB sans mode développeur (installe le pilote UsbDk)
french.DownloadingUsbDk=Téléchargement de UsbDk
french.UsbDkDownloadFailed=UsbDk n'a pas pu être téléchargé :%n%n%1%n%nCliquez sur Réessayer pour recommencer ou sur Annuler pour quitter. Vous pouvez aussi revenir en arrière et décocher la tâche USB.
french.UsbDkInstallFailed=UsbDk n'a pas pu être installé (code %1). L'USB sans mode développeur ne fonctionnera pas ; ezdisplay se rabat sur adb.
french.InstallingUsbDk=Installation du pilote UsbDk...
french.DownloadTitle=Téléchargement des composants
french.DownloadDesc=L'assistant télécharge les composants nécessaires : les bibliothèques vidéo FFmpeg (environ 90 Mo) et, si vous l'avez choisi, UsbDk.
french.ExtractingFfmpeg=Extraction de FFmpeg...
french.InstallingDriver=Installation du pilote d'affichage virtuel...
french.FfmpegDownloadFailed=FFmpeg n'a pas pu être téléchargé :%n%n%1%n%nCliquez sur Réessayer pour recommencer ou sur Annuler pour quitter.
french.FfmpegZipBadHash=Le fichier indiqué avec /FFMPEGZIP ne correspond pas à la somme SHA-256 attendue.
french.FfmpegInstallFailed=FFmpeg n'a pas pu être installé : %1
french.DriverFailed=Le pilote d'affichage virtuel n'a pas pu être installé (code %1).%n%nDétails : %2%n%nVoulez-vous réessayer ? Choisissez Non pour l'ignorer ; vous pourrez le réparer plus tard depuis les paramètres d'ezdisplay (Pilote d'affichage, Réparer).

brazilianportuguese.TaskDriver=Instalar o driver de tela virtual
brazilianportuguese.TaskAutostart=Iniciar o ezdisplay ao entrar no Windows
brazilianportuguese.TaskUsbDk=USB sem modo desenvolvedor (instala o driver UsbDk)
brazilianportuguese.DownloadingUsbDk=Baixando o UsbDk
brazilianportuguese.UsbDkDownloadFailed=Não foi possível baixar o UsbDk:%n%n%1%n%nClique em Repetir para tentar de novo ou em Cancelar para sair. Você também pode voltar e desmarcar a tarefa de USB.
brazilianportuguese.UsbDkInstallFailed=Não foi possível instalar o UsbDk (código %1). O USB sem modo desenvolvedor não vai funcionar; o ezdisplay recorre ao adb.
brazilianportuguese.InstallingUsbDk=Instalando o driver UsbDk...
brazilianportuguese.DownloadTitle=Baixando componentes
brazilianportuguese.DownloadDesc=O instalador está baixando os componentes necessários: as bibliotecas de vídeo do FFmpeg (cerca de 90 MB) e, se você escolheu, o UsbDk.
brazilianportuguese.ExtractingFfmpeg=Extraindo o FFmpeg...
brazilianportuguese.InstallingDriver=Instalando o driver de tela virtual...
brazilianportuguese.FfmpegDownloadFailed=Não foi possível baixar o FFmpeg:%n%n%1%n%nClique em Repetir para tentar de novo ou em Cancelar para sair.
brazilianportuguese.FfmpegZipBadHash=O arquivo informado em /FFMPEGZIP não corresponde à soma SHA-256 esperada.
brazilianportuguese.FfmpegInstallFailed=Não foi possível instalar o FFmpeg: %1
brazilianportuguese.DriverFailed=Não foi possível instalar o driver de tela virtual (código %1).%n%nDetalhes: %2%n%nQuer tentar de novo? Escolha Não para pular; você pode repará-lo depois nas configurações do ezdisplay (Driver de tela, Reparar).

italian.TaskDriver=Installa il driver dello schermo virtuale
italian.TaskAutostart=Avvia ezdisplay all'accesso
italian.TaskUsbDk=USB senza modalità sviluppatore (installa il driver UsbDk)
italian.DownloadingUsbDk=Download di UsbDk
italian.UsbDkDownloadFailed=Impossibile scaricare UsbDk:%n%n%1%n%nFai clic su Riprova per riprovare o su Annulla per uscire. Puoi anche tornare indietro e deselezionare l'attività USB.
italian.UsbDkInstallFailed=Impossibile installare UsbDk (codice %1). L'USB senza modalità sviluppatore non funzionerà; ezdisplay ripiega su adb.
italian.InstallingUsbDk=Installazione del driver UsbDk...
italian.DownloadTitle=Download dei componenti
italian.DownloadDesc=Il programma di installazione sta scaricando i componenti necessari: le librerie video FFmpeg (circa 90 MB) e, se l'hai scelto, UsbDk.
italian.ExtractingFfmpeg=Estrazione di FFmpeg...
italian.InstallingDriver=Installazione del driver dello schermo virtuale...
italian.FfmpegDownloadFailed=Impossibile scaricare FFmpeg:%n%n%1%n%nFai clic su Riprova per riprovare o su Annulla per uscire.
italian.FfmpegZipBadHash=Il file indicato con /FFMPEGZIP non corrisponde alla somma SHA-256 prevista.
italian.FfmpegInstallFailed=Impossibile installare FFmpeg: %1
italian.DriverFailed=Impossibile installare il driver dello schermo virtuale (codice %1).%n%nDettagli: %2%n%nVuoi riprovare? Scegli No per saltarlo; potrai ripararlo più tardi dalle impostazioni di ezdisplay (Driver dello schermo, Ripara).

german.TaskDriver=Treiber für den virtuellen Bildschirm installieren
german.TaskAutostart=ezdisplay bei der Anmeldung starten
german.TaskUsbDk=USB ohne Entwickleroptionen (installiert den UsbDk-Treiber)
german.DownloadingUsbDk=UsbDk wird heruntergeladen
german.UsbDkDownloadFailed=UsbDk konnte nicht heruntergeladen werden:%n%n%1%n%nKlicke auf Wiederholen, um es erneut zu versuchen, oder auf Abbrechen, um das Setup zu beenden. Du kannst auch zurückgehen und die USB-Aufgabe abwählen.
german.UsbDkInstallFailed=UsbDk konnte nicht installiert werden (Code %1). USB ohne Entwickleroptionen funktioniert nicht; ezdisplay weicht auf adb aus.
german.InstallingUsbDk=UsbDk-Treiber wird installiert...
german.DownloadTitle=Komponenten werden heruntergeladen
german.DownloadDesc=Das Setup lädt die benötigten Komponenten herunter: die FFmpeg-Videobibliotheken (etwa 90 MB) und, falls ausgewählt, UsbDk.
german.ExtractingFfmpeg=FFmpeg wird entpackt...
german.InstallingDriver=Treiber für den virtuellen Bildschirm wird installiert...
german.FfmpegDownloadFailed=FFmpeg konnte nicht heruntergeladen werden:%n%n%1%n%nKlicke auf Wiederholen, um es erneut zu versuchen, oder auf Abbrechen, um das Setup zu beenden.
german.FfmpegZipBadHash=Die mit /FFMPEGZIP angegebene Datei entspricht nicht der erwarteten SHA-256-Prüfsumme.
german.FfmpegInstallFailed=FFmpeg konnte nicht installiert werden: %1
german.DriverFailed=Der Treiber für den virtuellen Bildschirm konnte nicht installiert werden (Code %1).%n%nDetails: %2%n%nMöchtest du es erneut versuchen? Wähle Nein, um ihn zu überspringen; du kannst ihn später in den ezdisplay-Einstellungen reparieren (Bildschirmtreiber, Reparieren).

[Tasks]
Name: "driver"; Description: "{cm:TaskDriver}"
Name: "autostart"; Description: "{cm:TaskAutostart}"
; Optional, off by default: UsbDk lets the host open the tablet over USB without developer mode (AOA).
Name: "usbdk"; Description: "{cm:TaskUsbDk}"; Flags: unchecked
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#StagingDir}\ezdisplay.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StagingDir}\ezd-host.exe"; DestDir: "{app}"; Flags: ignoreversion
; Driver package (kept unchanged so the catalog signature stays valid) plus nefcon. Always copied,
; so the driver can be repaired later even if the task was skipped.
Source: "{#StagingDir}\driver\*"; DestDir: "{app}\driver"; Flags: ignoreversion recursesubdirs
Source: "{#StagingDir}\adb\*"; DestDir: "{app}\adb"; Flags: ignoreversion recursesubdirs
Source: "{#StagingDir}\licenses\*"; DestDir: "{app}\licenses"; Flags: ignoreversion recursesubdirs
; FFmpeg, extracted by [Code] at ssInstall from the downloaded archive (absent when the pinned build
; is already installed). Installed here rather than from [Code] so the DLLs exist before [Run]
; starts ezdisplay.exe (both executables import them) and the uninstall log tracks them.
Source: "{tmp}\ffmpeg\{#FfmpegDir}\bin\*.dll"; DestDir: "{app}"; Flags: external ignoreversion skipifsourcedoesntexist
Source: "{tmp}\ffmpeg\{#FfmpegDir}\LICENSE.txt"; DestDir: "{app}\licenses"; DestName: "FFmpeg-LICENSE.txt"; Flags: external ignoreversion skipifsourcedoesntexist

[InstallDelete]
; DLLs of a previously pinned FFmpeg build (their names carry the library major version).
Type: files; Name: "{app}\av*.dll"; Check: InstallingFfmpeg
Type: files; Name: "{app}\sw*.dll"; Check: InstallingFfmpeg

[Icons]
Name: "{autoprograms}\ezdisplay"; Filename: "{app}\ezdisplay.exe"
Name: "{autodesktop}\ezdisplay"; Filename: "{app}\ezdisplay.exe"; Tasks: desktopicon

[Run]
; Register the "ezdisplay" scheduled task that launches the app elevated (and at logon when the
; autostart task is selected). Registering it needs the installer's elevation, so these run
; elevated; the task targets the user at the console, not the account that approved UAC. The run
; level follows "run_elevated" in an existing config.toml (elevated by default).
Filename: "{app}\ezdisplay.exe"; Parameters: "--autostart on"; Tasks: autostart; Flags: runhidden waituntilterminated
Filename: "{app}\ezdisplay.exe"; Parameters: "--autostart off"; Tasks: not autostart; Flags: runhidden waituntilterminated
; Wi-Fi: tablets reach the host on UDP 27183. One inbound rule per executable, for the private
; profile only (a public network never gets the port). The delete first keeps a reinstall from
; stacking duplicates; it fails harmlessly on a first install.
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall delete rule name=""ezdisplay Wi-Fi (ezdisplay.exe)"""; Flags: runhidden waituntilterminated
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall delete rule name=""ezdisplay Wi-Fi (ezd-host.exe)"""; Flags: runhidden waituntilterminated
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall add rule name=""ezdisplay Wi-Fi (ezdisplay.exe)"" dir=in action=allow protocol=UDP localport=27183 profile=private program=""{app}\ezdisplay.exe"""; Flags: runhidden waituntilterminated
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall add rule name=""ezdisplay Wi-Fi (ezd-host.exe)"" dir=in action=allow protocol=UDP localport=27183 profile=private program=""{app}\ezd-host.exe"""; Flags: runhidden waituntilterminated
; Unelevated on purpose: the app asks the task to start its elevated copy.
Filename: "{app}\ezdisplay.exe"; Description: "{cm:LaunchProgram,ezdisplay}"; Flags: postinstall nowait runasoriginaluser skipifsilent

[UninstallRun]
; Order matters: stop the tray, delete its scheduled task, stop adb, then remove the driver, all
; before any file is deleted (the executables need the FFmpeg DLLs to start).
Filename: "{app}\ezdisplay.exe"; Parameters: "--quit"; RunOnceId: "QuitTray"; Flags: runhidden waituntilterminated
; The task launches the app elevated and carries the autostart trigger.
Filename: "{sys}\schtasks.exe"; Parameters: "/delete /tn ezdisplay /f"; RunOnceId: "DeleteTask"; Flags: runhidden waituntilterminated
; The Wi-Fi firewall rules of [Run].
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall delete rule name=""ezdisplay Wi-Fi (ezdisplay.exe)"""; RunOnceId: "FirewallTray"; Flags: runhidden waituntilterminated
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall delete rule name=""ezdisplay Wi-Fi (ezd-host.exe)"""; RunOnceId: "FirewallHost"; Flags: runhidden waituntilterminated
Filename: "{app}\adb\adb.exe"; Parameters: "kill-server"; RunOnceId: "KillAdb"; Flags: runhidden waituntilterminated
; Only removes the driver if ezdisplay installed it. Exit code 3010 (reboot required) cannot be
; read from [UninstallRun], so no restart prompt is shown for it.
Filename: "{app}\ezd-host.exe"; Parameters: "vdd uninstall --driver-dir ""{app}\driver"""; RunOnceId: "VddUninstall"; Flags: runhidden waituntilterminated

[UninstallDelete]
; The marker is written by [Code]; the DLL patterns are a safety net for the external [Files] entries.
Type: files; Name: "{app}\av*.dll"
Type: files; Name: "{app}\sw*.dll"
Type: files; Name: "{app}\ffmpeg.sha256"
Type: dirifempty; Name: "{app}\licenses"
Type: dirifempty; Name: "{app}"

[Code]
var
  DownloadPage: TDownloadWizardPage;
  // Path of the FFmpeg archive to install ('' = the pinned build is already installed).
  FfmpegZip: String;
  // Path of the downloaded UsbDk MSI ('' = nothing to install).
  UsbDkMsi: String;
  RestartNeeded: Boolean;

procedure InitializeWizard;
begin
  DownloadPage := CreateDownloadPage(CustomMessage('DownloadTitle'), CustomMessage('DownloadDesc'), nil);
end;

function NeedRestart(): Boolean;
begin
  Result := RestartNeeded;
end;

// The pinned build is installed when the marker holds its hash and its DLLs are there.
function FfmpegAlreadyInstalled: Boolean;
var
  Marker: AnsiString;
  FindRec: TFindRec;
begin
  Result := False;
  if not LoadStringFromFile(ExpandConstant('{app}\ffmpeg.sha256'), Marker) then
    Exit;
  if Pos('{#FfmpegSha256}', Lowercase(String(Marker))) = 0 then
    Exit;
  if FindFirst(ExpandConstant('{app}\avcodec-*.dll'), FindRec) then
  begin
    FindClose(FindRec);
    Result := True;
  end;
end;

// Check function of the [InstallDelete] entries: a new FFmpeg archive is being installed.
function InstallingFfmpeg: Boolean;
begin
  Result := FfmpegZip <> '';
end;

// UsbDk registers a kernel service named "UsbDk".
function UsbDkAlreadyInstalled: Boolean;
begin
  Result := RegKeyExists(HKLM64, 'SYSTEM\CurrentControlSet\Services\UsbDk');
end;

// Downloads the pinned UsbDk MSI when its task is selected and the driver is not installed yet.
// False means the user cancelled Setup after a failed download.
function DownloadUsbDk: Boolean;
var
  Retry: Boolean;
begin
  Result := True;
  UsbDkMsi := '';
  if not WizardIsTaskSelected('usbdk') then
    Exit;
  if UsbDkAlreadyInstalled then
  begin
    Log('ezdisplay: UsbDk already installed');
    Exit;
  end;
  repeat
    Retry := False;
    Log('ezdisplay: downloading UsbDk');
    DownloadPage.Clear;
    DownloadPage.SetText(CustomMessage('DownloadingUsbDk'), '');
    DownloadPage.Add('{#UsbDkUrl}', 'usbdk.msi', '{#UsbDkSha256}');
    DownloadPage.Show;
    try
      try
        DownloadPage.Download;
        UsbDkMsi := ExpandConstant('{tmp}\usbdk.msi');
      except
        Log('ezdisplay: UsbDk download failed: ' + GetExceptionMessage);
        if SuppressibleMsgBox(FmtMessage(CustomMessage('UsbDkDownloadFailed'), [GetExceptionMessage]),
          mbError, MB_RETRYCANCEL, IDCANCEL) = IDRETRY then
          Retry := True
        else
          Result := False;
      end;
    finally
      DownloadPage.Hide;
    end;
  until not Retry;
end;

// Runs on silent installs too (they simulate the Next clicks).
function NextButtonClick(CurPageID: Integer): Boolean;
var
  Given: String;
  Retry: Boolean;
begin
  Result := True;
  if CurPageID <> wpReady then
    Exit;

  if not DownloadUsbDk then
  begin
    Result := False;
    Exit;
  end;
  // Clear the UsbDk line of the progress page before FFmpeg uses it.
  DownloadPage.SetText('', '');

  FfmpegZip := '';
  if FfmpegAlreadyInstalled then
  begin
    Log('ezdisplay: FFmpeg already present');
    Exit;
  end;

  Given := ExpandConstant('{param:FFMPEGZIP|}');
  if Given <> '' then
  begin
    if (not FileExists(Given)) or (CompareText(GetSHA256OfFile(Given), '{#FfmpegSha256}') <> 0) then
    begin
      Log('ezdisplay: /FFMPEGZIP file missing or with a wrong checksum: ' + Given);
      SuppressibleMsgBox(CustomMessage('FfmpegZipBadHash'), mbCriticalError, MB_OK, IDOK);
      Result := False;
      Exit;
    end;
    Log('ezdisplay: using FFmpeg archive ' + Given);
    FfmpegZip := Given;
    Exit;
  end;

  repeat
    Retry := False;
    Log('ezdisplay: downloading FFmpeg');
    DownloadPage.Clear;
    DownloadPage.Add('{#FfmpegUrl}', 'ffmpeg.zip', '{#FfmpegSha256}');
    DownloadPage.Show;
    try
      try
        DownloadPage.Download;
        FfmpegZip := ExpandConstant('{tmp}\ffmpeg.zip');
      except
        Log('ezdisplay: FFmpeg download failed: ' + GetExceptionMessage);
        if SuppressibleMsgBox(FmtMessage(CustomMessage('FfmpegDownloadFailed'), [GetExceptionMessage]),
          mbError, MB_RETRYCANCEL, IDCANCEL) = IDRETRY then
          Retry := True
        else
          Result := False;
      end;
    finally
      DownloadPage.Hide;
    end;
  until not Retry;
end;

// Stops a running tray and the adb server it may have started, so files can be replaced.
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  ResultCode: Integer;
begin
  Result := '';
  if FileExists(ExpandConstant('{app}\ezdisplay.exe')) then
    ExecAsOriginalUser(ExpandConstant('{app}\ezdisplay.exe'), '--quit', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  if FileExists(ExpandConstant('{app}\adb\adb.exe')) then
    ExecAsOriginalUser(ExpandConstant('{app}\adb\adb.exe'), 'kill-server', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
end;

// Extracts the archive into {tmp}\ffmpeg, where the external [Files] entries pick up bin\*.dll and
// the license. Runs at ssInstall, before any file is installed.
function ExtractFfmpeg: Boolean;
var
  Work, BinDir: String;
  FindRec: TFindRec;
begin
  Result := False;
  Work := ExpandConstant('{tmp}\ffmpeg');
  BinDir := Work + '\{#FfmpegDir}\bin';
  WizardForm.StatusLabel.Caption := CustomMessage('ExtractingFfmpeg');
  try
    ForceDirectories(Work);
    ExtractArchive(FfmpegZip, Work, '', True, nil);
  except
    Log('ezdisplay: FFmpeg extraction failed: ' + GetExceptionMessage);
    SuppressibleMsgBox(FmtMessage(CustomMessage('FfmpegInstallFailed'), [GetExceptionMessage]), mbCriticalError, MB_OK, IDOK);
    Exit;
  end;
  if not FindFirst(BinDir + '\avcodec-*.dll', FindRec) then
  begin
    Log('ezdisplay: no FFmpeg DLLs in ' + BinDir);
    SuppressibleMsgBox(FmtMessage(CustomMessage('FfmpegInstallFailed'), ['bin\avcodec-*.dll']), mbCriticalError, MB_OK, IDOK);
    Exit;
  end;
  FindClose(FindRec);
  Log('ezdisplay: FFmpeg extracted to ' + Work);
  Result := True;
end;

// Windows shows its own "install this device software?" prompt during this step.
procedure InstallDriver;
var
  ResultCode: Integer;
  Finished: Boolean;
  LogPath: String;
begin
  LogPath := ExpandConstant('{commonappdata}\ezdisplay\logs\vdd-install.log');
  Finished := False;
  repeat
    WizardForm.StatusLabel.Caption := CustomMessage('InstallingDriver');
    if not Exec(ExpandConstant('{app}\ezd-host.exe'),
      'vdd install --driver-dir "' + ExpandConstant('{app}\driver') + '"', '', SW_HIDE, ewWaitUntilTerminated, ResultCode) then
      ResultCode := -1;
    Log('ezdisplay: vdd install exit code ' + IntToStr(ResultCode));
    if ResultCode = 0 then
      Finished := True
    else if ResultCode = 3010 then
    begin
      RestartNeeded := True;
      Finished := True;
    end
    else if SuppressibleMsgBox(FmtMessage(CustomMessage('DriverFailed'), [IntToStr(ResultCode), LogPath]),
      mbError, MB_YESNO, IDNO) <> IDYES then
      Finished := True;
  until Finished;
end;

// Silent MSI install of the UsbDk driver. A failure is not fatal: without UsbDk the host falls back
// to adb. Uninstall does not remove UsbDk on purpose, since other applications may depend on it.
procedure InstallUsbDk;
var
  ResultCode: Integer;
begin
  WizardForm.StatusLabel.Caption := CustomMessage('InstallingUsbDk');
  if not Exec(ExpandConstant('{sys}\msiexec.exe'), '/i "' + UsbDkMsi + '" /qn /norestart', '',
    SW_HIDE, ewWaitUntilTerminated, ResultCode) then
    ResultCode := -1;
  Log('ezdisplay: UsbDk msiexec exit code ' + IntToStr(ResultCode));
  if ResultCode = 3010 then
    RestartNeeded := True
  else if ResultCode <> 0 then
    SuppressibleMsgBox(FmtMessage(CustomMessage('UsbDkInstallFailed'), [IntToStr(ResultCode)]), mbError, MB_OK, IDOK);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  // ssInstall comes before [Files]; ssPostInstall after [Files] and the non-postinstall [Run].
  if CurStep = ssInstall then
  begin
    // Both executables need the FFmpeg DLLs: exit before anything is installed. ExtractFfmpeg has
    // already shown the reason, so use the silent Abort.
    if (FfmpegZip <> '') and not ExtractFfmpeg then
      Abort;
  end
  else if CurStep = ssPostInstall then
  begin
    if FfmpegZip <> '' then
    begin
      if SaveStringToFile(ExpandConstant('{app}\ffmpeg.sha256'), '{#FfmpegSha256}', False) then
        Log('ezdisplay: FFmpeg installed')
      else
        Log('ezdisplay: could not write ffmpeg.sha256; the next install downloads FFmpeg again');
    end;
    if WizardIsTaskSelected('driver') then
      InstallDriver;
    if UsbDkMsi <> '' then
      InstallUsbDk;
  end;
end;
