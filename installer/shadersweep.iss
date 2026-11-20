; Inno Setup script for Shadersweep.
; Build the exe first (cargo build --release), then compile this file with ISCC.
; Installs per user, so no admin prompt and the in-app updater can replace the exe.

#define AppName "Shadersweep"
#define AppExe "shadersweep.exe"
#define AppVersion GetStringFileInfo("..\target\release\shadersweep.exe", "ProductVersion")

[Setup]
; Keep this AppId unchanged across versions so upgrades replace the existing install.
AppId={{59F6D527-C88B-4D30-83EF-AE2DC9B5122A}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=etherloper
AppPublisherURL=https://github.com/etherloper/shadersweep
AppSupportURL=https://github.com/etherloper/shadersweep/issues
AppUpdatesURL=https://github.com/etherloper/shadersweep/releases
DefaultDirName={localappdata}\Programs\{#AppName}
DisableProgramGroupPage=yes
DisableDirPage=auto
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\target\installer
OutputBaseFilename=shadersweep-setup-{#AppVersion}
SetupIconFile=..\assets\shadersweep.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
RestartApplications=no

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "..\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; Leftovers from in-app updates. Settings in %APPDATA%\Shadersweep are kept.
Type: files; Name: "{app}\{#AppExe}.old"
Type: files; Name: "{app}\{#AppExe}.new"
