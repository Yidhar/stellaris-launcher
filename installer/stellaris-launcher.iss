; The Windows setup program (Inno Setup 7): installs the launcher for the current user, with no administrator rights, into
; %LOCALAPPDATA%\Programs\Stellaris Launcher (a folder the launcher can update itself in), adds it to the Start menu and to
; "Apps & features" with an uninstaller, and on uninstall asks whether to keep the playsets and settings.
;
; Built by scripts/package_installer.ps1 (which passes the version and the folder of the release build):
;     iscc /DAppVersion=0.1.0 /DSourceDir=<repo>\target\release installer\stellaris-launcher.iss

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\target\release"
#endif

[Setup]
; the launcher looks for this entry (Uninstall\StellarisLauncher_is1) to tell a setup install from a copy unpacked from the zip
AppId=StellarisLauncher
AppName=Stellaris Launcher
AppVersion={#AppVersion}
AppVerName=Stellaris Launcher {#AppVersion}
AppPublisher=Yidhar
AppPublisherURL=https://github.com/Yidhar/stellaris-launcher
AppSupportURL=https://github.com/Yidhar/stellaris-launcher/issues
AppUpdatesURL=https://github.com/Yidhar/stellaris-launcher/releases
VersionInfoVersion={#AppVersion}
DefaultDirName={localappdata}\Programs\Stellaris Launcher
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
OutputDir=..\package
OutputBaseFilename=stellaris-launcher-setup-v{#AppVersion}
SetupIconFile=..\crates\stl-gui\assets\stellaris-launcher.ico
UninstallDisplayIcon={app}\stellaris-launcher.exe
UninstallDisplayName=Stellaris Launcher
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
CloseApplications=yes
RestartApplications=no
ShowLanguageDialog=auto
LicenseFile=..\LICENSE

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "zh_hans"; MessagesFile: "compiler:Languages\ChineseSimplified.isl"
Name: "zh_hant"; MessagesFile: "compiler:Languages\ChineseTraditional.isl"
Name: "ja"; MessagesFile: "compiler:Languages\Japanese.isl"
Name: "ko"; MessagesFile: "compiler:Languages\Korean.isl"
Name: "de"; MessagesFile: "compiler:Languages\German.isl"
Name: "fr"; MessagesFile: "compiler:Languages\French.isl"
Name: "es"; MessagesFile: "compiler:Languages\Spanish.isl"
Name: "ru"; MessagesFile: "compiler:Languages\Russian.isl"

[CustomMessages]
en.DeleteData=Also delete your playsets and settings (%1)?%n%nChoose No to keep them for a later install.
zh_hans.DeleteData=是否同时删除你的播放集和设置（%1）？%n%n选“否”会保留它们，以后重新安装还能用。
zh_hant.DeleteData=是否同時刪除你的播放集和設定（%1）？%n%n選「否」會保留它們，日後重新安裝仍可使用。
ja.DeleteData=プレイセットと設定（%1）も削除しますか？%n%n「いいえ」で残しておけば再インストール時に使えます。
ko.DeleteData=플레이셋과 설정(%1)도 삭제할까요?%n%n'아니요'를 고르면 다시 설치할 때 쓸 수 있도록 남겨 둡니다.
de.DeleteData=Auch Playsets und Einstellungen (%1) löschen?%n%nMit Nein bleiben sie für eine spätere Installation erhalten.
fr.DeleteData=Supprimer aussi vos playsets et réglages (%1) ?%n%nNon les garde pour une prochaine installation.
es.DeleteData=¿Borrar también tus playsets y ajustes (%1)?%n%nNo los conserva para una instalación posterior.
ru.DeleteData=Удалить также ваши наборы и настройки (%1)?%n%n«Нет» — сохранить их для будущей установки.

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceDir}\stellaris-launcher.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\stl.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\docs\*.md"; DestDir: "{app}\docs"; Flags: ignoreversion
Source: "..\examples\*"; DestDir: "{app}\examples"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\Stellaris Launcher"; Filename: "{app}\stellaris-launcher.exe"
Name: "{autodesktop}\Stellaris Launcher"; Filename: "{app}\stellaris-launcher.exe"; Tasks: desktopicon

[Registry]
; Win+R "stellaris-launcher"
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\App Paths\stellaris-launcher.exe"; ValueType: string; ValueName: ""; ValueData: "{app}\stellaris-launcher.exe"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\App Paths\stellaris-launcher.exe"; ValueType: string; ValueName: "Path"; ValueData: "{app}"

[Run]
Filename: "{app}\stellaris-launcher.exe"; Description: "{cm:LaunchProgram,Stellaris Launcher}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; what the launcher's self-update leaves next to the files it replaced, until its next start
Type: files; Name: "{app}\*.old"
Type: dirifempty; Name: "{app}"

[Code]
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Data: String;
begin
  if CurUninstallStep = usPostUninstall then
  begin
    Data := ExpandConstant('{userappdata}\stellaris-launcher');
    if DirExists(Data) and not UninstallSilent() then
      if MsgBox(FmtMessage(CustomMessage('DeleteData'), [Data]), mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
        DelTree(Data, True, True, True);
  end;
end;
