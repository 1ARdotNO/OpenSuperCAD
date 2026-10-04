; Inno Setup script for the OpenSuperCAD Windows installer.
; Built by release.yml: iscc /DVersion=<x.y.z> /DSource=<folder with the .exe files> opensupercad.iss

#ifndef Version
  #define Version "0.0.0"
#endif
#ifndef Source
  #define Source "..\..\target\release"
#endif

[Setup]
AppId={{8F390DBC-475D-40C8-B7CA-EF6152C07C5D}
AppName=OpenSuperCAD
AppVersion={#Version}
AppPublisher=OpenSuperCAD contributors
AppPublisherURL=https://github.com/1ARdotNO/OpenSuperCAD
AppSupportURL=https://github.com/1ARdotNO/OpenSuperCAD/issues
AppUpdatesURL=https://github.com/1ARdotNO/OpenSuperCAD/releases
DefaultDirName={autopf}\OpenSuperCAD
DefaultGroupName=OpenSuperCAD
DisableProgramGroupPage=yes
; Install for the current user by default (no admin prompt); allow all users.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputBaseFilename=OpenSuperCAD-{#Version}-windows-x86_64-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
LicenseFile=..\..\LICENSE-MIT
UninstallDisplayName=OpenSuperCAD {#Version}
SetupIconFile=opensupercad.ico
UninstallDisplayIcon={app}\opensupercad.exe

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#Source}\opensupercad.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Source}\opensupercad-mcp.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE-MIT"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE-APACHE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\README.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\OpenSuperCAD"; Filename: "{app}\opensupercad.exe"
Name: "{autodesktop}\OpenSuperCAD"; Filename: "{app}\opensupercad.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\opensupercad.exe"; Description: "{cm:LaunchProgram,OpenSuperCAD}"; Flags: nowait postinstall skipifsilent
; In-app updates run the installer silently with /relaunch=1 to start the new
; version afterwards. Other silent installs (winget) don't start the app.
Filename: "{app}\opensupercad.exe"; Flags: nowait; Check: ShouldRelaunch

[Code]
function ShouldRelaunch: Boolean;
begin
  Result := WizardSilent and (ExpandConstant('{param:relaunch|0}') = '1');
end;
