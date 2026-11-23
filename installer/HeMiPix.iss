; HeMiPix installer (Inno Setup 6), built by release.ps1:  ISCC /DAppVersion=1.0.0 /DAppExe=<HeMiPix.exe> /DOutputDir=<folder>
; For the current user (no administrator) or for all users - the installer asks. Uninstalling removes the File Explorer item of the app as well.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef AppExe
  #define AppExe "..\build\release\HeMiPix.exe"
#endif
#ifndef AppExeArm
  #define AppExeArm ""
#endif
#ifndef OutputDir
  #define OutputDir "..\build\dist"
#endif

[Setup]
AppId=HeMiPix
AppName=HeMiPix
AppVersion={#AppVersion}
AppVerName=HeMiPix {#AppVersion}
AppPublisher=Richard Hemala
AppPublisherURL=https://github.com/Hemal1k/HeMiPix
AppSupportURL=https://github.com/Hemal1k/HeMiPix/issues
AppUpdatesURL=https://github.com/Hemal1k/HeMiPix/releases
DefaultDirName={autopf}\HeMi\HeMiPix
UsePreviousAppDir=no
DisableDirPage=yes
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
; the first page asks: for me only (default, no administrator) or for all users (Program Files, asks for administrator)
PrivilegesRequiredOverridesAllowed=dialog commandline
ArchitecturesAllowed=x64compatible or arm64
ArchitecturesInstallIn64BitMode=x64compatible or arm64
MinVersion=10.0
OutputDir={#OutputDir}
OutputBaseFilename=HeMiPix-Setup
SetupIconFile=..\assets\HeMiPix.ico
UninstallDisplayIcon={app}\HeMiPix.exe
UninstallDisplayName=HeMiPix
Compression=lzma2/max
SolidCompression=yes
; the look of Windows 11, light or dark as Windows, and as big as the first question of the installer (it was
; smaller and old-looking next to it, 2026-11-19)
WizardStyle=modern dynamic windows11
WizardSizePercent=120
CloseApplications=force
LicenseFile=..\LICENSE

[Languages]
; English only
Name: "en"; MessagesFile: "compiler:Default.isl"

[Files]
; x64, and the ARM64 program on Windows on ARM when the installer has it
#if AppExeArm != ""
Source: "{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion; Check: not IsArm64
Source: "{#AppExeArm}"; DestDir: "{app}"; DestName: "HeMiPix.exe"; Flags: ignoreversion; Check: IsArm64
#else
Source: "{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
#endif

[Tasks]
; offered on its own page, off by default
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Icons]
Name: "{autoprograms}\HeMiPix"; Filename: "{app}\HeMiPix.exe"
Name: "{autodesktop}\HeMiPix"; Filename: "{app}\HeMiPix.exe"; Tasks: desktopicon

[Run]
; the File Explorer items and the start with Windows point to this installation (also after a move from the old folder)
Filename: "{app}\HeMiPix.exe"; Parameters: "--repair"; Flags: runasoriginaluser waituntilterminated runhidden
Filename: "{app}\HeMiPix.exe"; Description: "{cm:LaunchProgram,HeMiPix}"; Flags: nowait postinstall skipifsilent runasoriginaluser

[Registry]
; the File Explorer item the app writes when it is switched on: removed with the app
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.heic\shell\HeMiPix"; Flags: uninsdeletekey dontcreatekey
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.heif\shell\HeMiPix"; Flags: uninsdeletekey dontcreatekey
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.hif\shell\HeMiPix"; Flags: uninsdeletekey dontcreatekey
Root: HKCU; Subkey: "Software\Classes\Directory\shell\HeMiPix"; Flags: uninsdeletekey dontcreatekey
; what Windows itself keeps about the app (its name in menus, "Open with")
Root: HKCU; Subkey: "Software\Classes\Local Settings\Software\Microsoft\Windows\Shell\MuiCache"; ValueType: none; ValueName: "{app}\HeMiPix.exe.FriendlyAppName"; Flags: uninsdeletevalue dontcreatekey
Root: HKCU; Subkey: "Software\Classes\Local Settings\Software\Microsoft\Windows\Shell\MuiCache"; ValueType: none; ValueName: "{app}\HeMiPix.exe.ApplicationCompany"; Flags: uninsdeletevalue dontcreatekey
Root: HKCU; Subkey: "Software\Classes\Applications\HeMiPix.exe"; Flags: uninsdeletekey dontcreatekey
Root: HKCU; Subkey: "Software\HeMi"; Flags: uninsdeletekeyifempty dontcreatekey

[CustomMessages]
ExtTitle=Photo extensions from Microsoft
ExtDesc=HEIC photos are read by Windows itself, with two free extensions.
ExtText=These are missing on this PC (without them HeMiPix converts everything but HEIC). After the installation HEIF comes from the Microsoft Store and the store page of HEVC opens. Their license terms:
ExtLicense=license terms
ExtAccept=I accept the license terms and want them installed

[Code]
// Since 2026-11 the HeMi apps go into one folder HeMi, as their settings do (%AppData%\HeMi\<app>): {autopf}\HeMi\HeMiPix.
// An installation in the old folder {autopf}\HeMiPix moves: the app running from there ends, the new version goes into the
// new folder ("--repair" points the File Explorer items and the start with Windows to it) and the old folder is deleted.
var
  OldFolder: String;

function FindOldFolder: String;
begin
  if RegQueryStringValue(HKA, 'Software\Microsoft\Windows\CurrentVersion\Uninstall\HeMiPix_is1', 'InstallLocation', Result) then
  begin
    Result := RemoveBackslashUnlessRoot(Result);
    if (CompareText(Result, ExpandConstant('{app}')) = 0) or (CompareText(ExtractFileName(Result), 'HeMiPix') <> 0)
      or not FileExists(Result + '\HeMiPix.exe') then
      Result := '';
  end
  else
    Result := '';
end;

procedure StopOld;
var
  Code: Integer;
begin
  OldFolder := FindOldFolder;
  if OldFolder = '' then Exit;
  Exec(ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'),
    '-NoProfile -NonInteractive -Command "Get-Process | Where-Object { $_.Path -like ''' + OldFolder + '\*'' } | Stop-Process -Force"',
    '', SW_HIDE, ewWaitUntilTerminated, Code);
end;

procedure DeleteOld;
begin
  if OldFolder <> '' then DelTree(OldFolder, True, True, True);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  Result := '';
  StopOld;
end;

const
  HeifId = '9PMMSR1CGPWG';
  HevcId = '9N4WGH0Z6VHQ';
  StoreLicense = 'https://go.microsoft.com/fwlink/?LinkId=822631';
  HevcPage = 'https://apps.microsoft.com/detail/9N4WGH0Z6VHQ';

var
  ExtPage: TWizardPage;
  ExtAccept: TNewCheckBox;
  NeedHeif, NeedHevc: Boolean;

{ an app package of the Microsoft Store is installed for the user (asked by PowerShell) }
function HasAppx(const Name: String): Boolean;
var
  Code: Integer;
begin
  Result := Exec('powershell.exe', '-NoProfile -ExecutionPolicy Bypass -Command "if (Get-AppxPackage -Name ' + Name + ') { exit 0 } else { exit 1 }"',
    '', SW_HIDE, ewWaitUntilTerminated, Code) and (Code = 0);
end;

procedure OpenLink(Sender: TObject);
var
  Code: Integer;
begin
  ShellExec('open', TNewStaticText(Sender).Hint, '', '', SW_SHOWNORMAL, ewNoWait, Code);
end;

function AddExtension(const Name, License: String; Top: Integer): Integer;
var
  N, L: TNewStaticText;
begin
  N := TNewStaticText.Create(ExtPage);
  N.Parent := ExtPage.Surface;
  N.Top := Top;
  N.Left := ScaleX(8);
  N.Caption := '•  ' + Name + '  –  ';
  N.Font.Style := [fsBold];
  L := TNewStaticText.Create(ExtPage);
  L.Parent := ExtPage.Surface;
  L.Top := Top;
  L.Left := N.Left + N.Width;
  L.Caption := CustomMessage('ExtLicense');
  L.Hint := License;
  L.Cursor := crHand;
  L.Font.Color := clHotLight;
  L.Font.Style := [fsUnderline];
  L.OnClick := @OpenLink;
  Result := Top + L.Height + ScaleY(8);
end;

procedure InitializeWizard;
var
  T: TNewStaticText;
  Y: Integer;
begin
  NeedHeif := not HasAppx('Microsoft.HEIFImageExtension');
  NeedHevc := not HasAppx('Microsoft.HEVCVideoExtension*');
#ifdef TestExtPage
  NeedHeif := True; NeedHevc := True;   { ISCC /DTestExtPage: shows the page on a PC that has them }
#endif
  ExtPage := CreateCustomPage(wpSelectTasks, CustomMessage('ExtTitle'), CustomMessage('ExtDesc'));
  T := TNewStaticText.Create(ExtPage);
  T.Parent := ExtPage.Surface;
  T.Width := ExtPage.SurfaceWidth;
  T.WordWrap := True;
  T.AutoSize := True;
  T.Caption := CustomMessage('ExtText');
  Y := T.Top + T.Height + ScaleY(16);
  if NeedHeif then Y := AddExtension('HEIF Image Extensions', StoreLicense, Y);
  if NeedHevc then Y := AddExtension('HEVC Video Extensions', HevcPage, Y);
  ExtAccept := TNewCheckBox.Create(ExtPage);
  ExtAccept.Parent := ExtPage.Surface;
  ExtAccept.Top := Y + ScaleY(12);
  ExtAccept.Width := ExtPage.SurfaceWidth;
  ExtAccept.Height := ScaleY(22);
  ExtAccept.Caption := CustomMessage('ExtAccept');
end;

function ShouldSkipPage(PageID: Integer): Boolean;
begin
  Result := (PageID = ExtPage.ID) and not (NeedHeif or NeedHevc);
end;

{ after the installation, as the user (the extensions are installed for the user): both by their install windows of the Microsoft Store }
procedure CurStepChanged(CurStep: TSetupStep);
var
  Code: Integer;
begin
  if CurStep = ssPostInstall then DeleteOld;
  if (CurStep <> ssPostInstall) or WizardSilent or not ExtAccept.Checked then Exit;
  if NeedHeif then
    ShellExecAsOriginalUser('open', 'ms-windows-store://pdp/?ProductId=' + HeifId + '&mode=mini', '', '', SW_SHOWNORMAL, ewNoWait, Code);
  if NeedHevc then
    ShellExecAsOriginalUser('open', 'ms-windows-store://pdp/?ProductId=' + HevcId + '&mode=mini', '', '', SW_SHOWNORMAL, ewNoWait, Code);
end;

// Uninstalling: the settings of the app (%AppData%\HeMi\HeMiPix) are deleted or kept, as the user chooses (a quiet
// uninstall keeps them). %AppData%\HeMi goes too when no other HeMi app has anything there.
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Dir: String;
begin
  if CurUninstallStep <> usPostUninstall then Exit;
  Dir := ExpandConstant('{userappdata}\HeMi\HeMiPix');
  if not DirExists(Dir) then Exit;
  if UninstallSilent then Exit;
  if MsgBox('Delete the settings of HeMiPix too?' + #13#10#13#10 + 'Keep them to use them again when you install HeMiPix later.', mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
  begin
    DelTree(Dir, True, True, True);
    RemoveDir(ExpandConstant('{userappdata}\HeMi'));
  end;
end;
