; The crest installer for Windows (Inno Setup 6), built by
; scripts/package.ps1 -Installer:
;
;   ISCC.exe /DAppVersion=0.14.0 /DAppVersionNumbers=0.14.0.0
;            /DSourceDir=<dir with crest.exe, README.txt, LICENSE...>
;            /DOutDir=dist /DOutputName=crest-0.14.0-x86_64-pc-windows-msvc-setup
;            packaging\windows\crest.iss
;
; Per user by default, no administrator rights: crest.exe in
; %LOCALAPPDATA%\Programs\crest (where scripts/install.ps1 put it, so an
; install made by the script is upgraded in place), a Start menu entry, an
; optional desktop icon, the folder on the user PATH, and an entry in
; Settings > Apps with the icon, the version and the uninstaller. "Install
; for all users" (the dialog or /ALLUSERS) uses Program Files and the
; system PATH instead. A newer setup upgrades over the same AppId, closing
; a running crest first. The config and data folders stay on uninstall.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef AppVersionNumbers
  #define AppVersionNumbers "0.0.0.0"
#endif
#ifndef SourceDir
  #error SourceDir is required: the folder with crest.exe
#endif
#ifndef OutDir
  #define OutDir "dist"
#endif
#ifndef OutputName
  #define OutputName "crest-setup"
#endif

[Setup]
; Never change it: Windows finds the installed crest by this id.
AppId={{9C1A9DD2-A2E2-4F50-BA98-5C1E587342BE}
AppName=crest
AppVersion={#AppVersion}
AppVerName=crest {#AppVersion}
AppPublisher=TwoCrest Systems
AppPublisherURL=https://github.com/lazaroMB/crest
AppSupportURL=https://github.com/lazaroMB/crest/issues
AppUpdatesURL=https://github.com/lazaroMB/crest/releases
AppCopyright=GPL-3.0-or-later
VersionInfoVersion={#AppVersionNumbers}
DefaultDirName={autopf}\crest
DisableDirPage=auto
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog commandline
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Windows 10 1809, as the README says.
MinVersion=10.0.17763
SetupIconFile=..\..\assets\windows\crest.ico
UninstallDisplayIcon={app}\crest.exe
UninstallDisplayName=crest
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
ChangesEnvironment=yes
CloseApplications=yes
RestartApplications=no
OutputDir={#OutDir}
OutputBaseFilename={#OutputName}

[Tasks]
Name: "addtopath"; Description: "Add crest to the PATH (the ""crest"" command in terminals)"
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceDir}\crest.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\README.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\THIRD-PARTY.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\crest"; Filename: "{app}\crest.exe"; WorkingDir: "{%USERPROFILE}"; Comment: "crest, a coding agent"
Name: "{autodesktop}\crest"; Filename: "{app}\crest.exe"; WorkingDir: "{%USERPROFILE}"; Comment: "crest, a coding agent"; Tasks: desktopicon

[Run]
Filename: "{app}\crest.exe"; WorkingDir: "{%USERPROFILE}"; Description: "{cm:LaunchProgram,crest}"; Flags: nowait postinstall skipifsilent

[Code]
// The PATH of the user, or the system's when installing for all users.
function EnvRoot: Integer;
begin
  if IsAdminInstallMode then
    Result := HKEY_LOCAL_MACHINE
  else
    Result := HKEY_CURRENT_USER;
end;

function EnvKey: String;
begin
  if IsAdminInstallMode then
    Result := 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment'
  else
    Result := 'Environment';
end;

// Position of ;Dir; in ;Paths; ignoring case, 0 when absent.
function PathPos(Paths, Dir: String): Integer;
begin
  Result := Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(Paths) + ';');
end;

procedure AddToPath(Dir: String);
var
  Paths: String;
begin
  if not RegQueryStringValue(EnvRoot, EnvKey, 'Path', Paths) then
    Paths := '';
  if PathPos(Paths, Dir) > 0 then
    exit;
  if (Paths <> '') and (Copy(Paths, Length(Paths), 1) <> ';') then
    Paths := Paths + ';';
  RegWriteExpandStringValue(EnvRoot, EnvKey, 'Path', Paths + Dir);
end;

procedure RemoveFromPath(Dir: String);
var
  Paths: String;
  P: Integer;
begin
  if not RegQueryStringValue(EnvRoot, EnvKey, 'Path', Paths) then
    exit;
  P := PathPos(Paths, Dir);
  if P = 0 then
    exit;
  Paths := ';' + Paths + ';';
  Delete(Paths, P, Length(Dir) + 1);
  Paths := Copy(Paths, 2, Length(Paths) - 2);
  RegWriteExpandStringValue(EnvRoot, EnvKey, 'Path', Paths);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if (CurStep = ssPostInstall) and WizardIsTaskSelected('addtopath') then
    AddToPath(ExpandConstant('{app}'));
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
    RemoveFromPath(ExpandConstant('{app}'));
end;
