; Orbit's installer: a normal Windows setup wizard, for the current user, with no admin rights.
;
; Build it with scripts/package-windows.ps1, which passes the version and the built exe:
;   ISCC.exe /DAppVersion=1.0.0 /DSourceExe=..\target\release\orbit.exe installer\orbit.iss

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceExe
  #define SourceExe "..\target\release\orbit.exe"
#endif

#define AppName "Orbit"
#define AppExe "orbit.exe"

[Setup]
; Never change this: Windows knows the installed app by it, for upgrades and uninstalling.
AppId={{2124CE11-6877-42C6-B33E-D4C37E050DE4}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=Beroris
AppPublisherURL=https://github.com/Beroris/orbit
AppSupportURL=https://github.com/Beroris/orbit/issues
AppComments=A fast, keyboard-first launcher.
VersionInfoVersion={#AppVersion}
VersionInfoDescription={#AppName} Setup
; Per user: no admin prompt.
PrivilegesRequired=lowest
DefaultDirName={localappdata}\Programs\{#AppName}
DisableDirPage=yes
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
DisableWelcomePage=no
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
SetupIconFile=..\platforms\windows\res\orbit.ico
WizardStyle=modern
WizardSizePercent=100
WizardSmallImageFile=..\assets\icon.png
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.17763
Compression=lzma2/max
SolidCompression=yes
OutputDir=..\dist
OutputBaseFilename=Orbit-{#AppVersion}-Setup
; Orbit is closed by the code below, so Windows' "close these apps" page isn't needed.
CloseApplications=no
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "autostart"; Description: "Start {#AppName} when I sign in"; GroupDescription: "Options:"
Name: "desktopicon"; Description: "Put a shortcut on the desktop"; GroupDescription: "Options:"; Flags: unchecked

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; DestName: "{#AppExe}"; Flags: ignoreversion
Source: "..\docs\PRIVACY.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\docs\THIRD_PARTY_NOTICES.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\assets\fonts\Geist-LICENSE.txt"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"; Comment: "Open the launcher"
Name: "{userdesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Comment: "Open the launcher"; Tasks: desktopicon

[Registry]
; The same entry Orbit's own "Start Orbit when I sign in" switch writes, so the two agree.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "{#AppName}"; ValueData: """{app}\{#AppExe}"""; Tasks: autostart
; Uninstalling removes it in the code below, only if it still points here.

[Run]
Filename: "{app}\{#AppExe}"; Description: "Start {#AppName} now"; Flags: postinstall nowait skipifsilent

[UninstallDelete]
Type: dirifempty; Name: "{app}"

[Code]
const
  RunKey = 'Software\Microsoft\Windows\CurrentVersion\Run';

{ Asks a running Orbit to quit, so its exe can be replaced or removed and its hotkeys and keyboard
  hook let go. }
procedure QuitOrbit();
var
  Code: Integer;
  Exe: String;
begin
  Exe := ExpandConstant('{app}\{#AppExe}');
  if FileExists(Exe) then
  begin
    Exec(Exe, '--quit', '', SW_HIDE, ewWaitUntilTerminated, Code);
    Sleep(300);
  end;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  QuitOrbit();
  Result := '';
end;

{ The sign-in entry points at this copy, or at none. }
function PointsHere(): Boolean;
var
  Value: String;
begin
  Result := RegQueryStringValue(HKCU, RunKey, '{#AppName}', Value) and
    (CompareText(RemoveQuotes(Value), ExpandConstant('{app}\{#AppExe}')) = 0);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  { Unticking the sign-in option on an upgrade turns it off, as it says. }
  if (CurStep = ssPostInstall) and not WizardIsTaskSelected('autostart') and PointsHere() then
    RegDeleteValue(HKCU, RunKey, '{#AppName}');
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Data: String;
begin
  if CurUninstallStep = usUninstall then
  begin
    QuitOrbit();
    { Only Orbit's own entry. A copy of Orbit kept elsewhere keeps its own. }
    if PointsHere() then
      RegDeleteValue(HKCU, RunKey, '{#AppName}');
  end;
  Data := ExpandConstant('{localappdata}\{#AppName}');
  if (CurUninstallStep = usPostUninstall) and DirExists(Data) and not UninstallSilent() then
  begin
    if MsgBox('Also remove your Orbit settings, notes, snippets and clipboard history?' #13#10#13#10 'Keep them if you might install Orbit again.', mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
      DelTree(Data, True, True, True);
  end;
end;
