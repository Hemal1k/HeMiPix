; HeMiPix installer (Inno Setup 6), built by release.ps1:  ISCC /DAppVersion=1.0.0 /DAppExe=<HeMiPix.exe> /DOutputDir=<folder>
; For the current user (no administrator) or for all users - the installer asks. Uninstalling removes the File Explorer item of the app as well.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef AppExe
  #define AppExe "..\build\release\HeMiPix.exe"
#endif
#ifndef OutputDir
  #define OutputDir "..\build\dist"
#endif

[Setup]
AppId=HeMiPix
AppName=HeMiPix
AppVersion={#AppVersion}
AppVerName=HeMiPix Beta {#AppVersion}
AppPublisher=Richard Hemala
AppPublisherURL=https://github.com/Hemal1k/HeMiPix
AppSupportURL=https://github.com/Hemal1k/HeMiPix/issues
AppUpdatesURL=https://github.com/Hemal1k/HeMiPix/releases
DefaultDirName={autopf}\HeMiPix
DisableDirPage=yes
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
; the first page asks: for me only (default, no administrator) or for all users (Program Files, asks for administrator)
PrivilegesRequiredOverridesAllowed=dialog commandline
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir={#OutputDir}
OutputBaseFilename=HeMiPix-Setup
SetupIconFile=..\assets\HeMiPix.ico
UninstallDisplayIcon={app}\HeMiPix.exe
UninstallDisplayName=HeMiPix
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
CloseApplications=force
LicenseFile=..\LICENSE

[Languages]
; English only
Name: "en"; MessagesFile: "compiler:Default.isl"

[Files]
Source: "{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion

[Tasks]
; offered on its own page, off by default
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Icons]
Name: "{autoprograms}\HeMiPix"; Filename: "{app}\HeMiPix.exe"
Name: "{autodesktop}\HeMiPix"; Filename: "{app}\HeMiPix.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\HeMiPix.exe"; Description: "{cm:LaunchProgram,HeMiPix}"; Flags: nowait postinstall skipifsilent runasoriginaluser

[Registry]
; the File Explorer item the app writes when it is switched on: removed with the app
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.heic\shell\HeMiPix"; Flags: uninsdeletekey dontcreatekey
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.heif\shell\HeMiPix"; Flags: uninsdeletekey dontcreatekey
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.hif\shell\HeMiPix"; Flags: uninsdeletekey dontcreatekey
Root: HKCU; Subkey: "Software\Classes\Directory\shell\HeMiPix"; Flags: uninsdeletekey dontcreatekey
