; Nowhere Player 的 per-user Inno Setup 安装器。
;
; 由 scripts/build-installer.ps1 传入：
;   /DPackageDir=<release/NowherePlayer-...>
;   /DMyAppVersion=<Cargo.toml version>
;   /DOutputDir=<release>
;
; 不写 HKLM、不装服务、不请求管理员权限。安装后 exe 不再带浏览器下载的
; Mark-of-the-Web，通常不会每次启动都触发 SmartScreen；安装器自身未签名时
; 首次运行仍可能提示一次。

#ifndef MyAppVersion
  #define MyAppVersion "0.1.0"
#endif

#ifndef PackageDir
  #define PackageDir "..\..\release\NowherePlayer-t0121"
#endif

#ifndef OutputDir
  #define OutputDir "..\..\release"
#endif

#define MyAppName "Nowhere Player"
#define MyAppPublisher "NowherePlayer"
#define MyAppExeName "np-tui.exe"
#define MyAppId "{{8F2F4D3C-9B7A-4E57-9F1B-0C5F6E2A7D41}"

[Setup]
AppId={#MyAppId}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppVerName={#MyAppName} {#MyAppVersion}
AppPublisher={#MyAppPublisher}
DefaultDirName={localappdata}\Programs\NowherePlayer
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
DisableDirPage=no
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#OutputDir}
OutputBaseFilename=NowherePlayer-Setup-{#MyAppVersion}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
UninstallDisplayIcon={app}\np-tui.exe
UninstallDisplayName={#MyAppName}
SetupLogging=yes
DirExistsWarning=no
UsePreviousAppDir=yes
MinVersion=10.0

[Languages]
Name: "default"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "创建桌面快捷方式"; GroupDescription: "附加任务："; Flags: unchecked

[Files]
Source: "{#PackageDir}\np-tui.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PackageDir}\np-video.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PackageDir}\mpv\*"; DestDir: "{app}\mpv"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; WorkingDir: "{app}"
Name: "{group}\卸载 {#MyAppName}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; WorkingDir: "{app}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "启动 {#MyAppName}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; 用户数据在 %APPDATA%\com.nowhere.player，卸载器不删除，保留播放列表/设置。
Type: filesandordirs; Name: "{app}\mpv"
