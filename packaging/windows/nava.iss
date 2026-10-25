; Inno Setup script. CI passes /DAppVersion=... and /DSrcDir=...
#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SrcDir
  #define SrcDir "..\..\dist\nava"
#endif

#define SysEnv "SYSTEM\CurrentControlSet\Control\Session Manager\Environment"

[Setup]
AppId={{6F2B3C1E-6A1D-4E3B-9B8E-4D7A0C2F9A11}
AppName=Nava
AppVersion={#AppVersion}
AppPublisher=Nava
DefaultDirName={autopf}\Nava
DefaultGroupName=Nava
DisableProgramGroupPage=yes
PrivilegesRequiredOverridesAllowed=dialog
OutputBaseFilename=nava-windows-x64-setup
SetupIconFile=nava.ico
UninstallDisplayIcon={app}\nava.exe
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
ChangesEnvironment=yes

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "addtopath"; Description: "Add nava to PATH (for the command line)"; Flags: unchecked

[Files]
Source: "{#SrcDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs

[Icons]
Name: "{group}\Nava"; Filename: "{app}\nava.exe"
Name: "{autodesktop}\Nava"; Filename: "{app}\nava.exe"; Tasks: desktopicon

[Registry]
; PATH lives in different places for "just me" and "all users" installs.
Root: HKCU; Subkey: "Environment"; ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; Tasks: addtopath; Check: (not IsAdminInstallMode) and NeedsAddPath(ExpandConstant('{app}'))
Root: HKLM; Subkey: "{#SysEnv}"; ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; Tasks: addtopath; Check: IsAdminInstallMode and NeedsAddPath(ExpandConstant('{app}'))

[Run]
Filename: "{app}\nava.exe"; Description: "{cm:LaunchProgram,Nava}"; Flags: nowait postinstall skipifsilent

[Code]
procedure EnvKey(var Root: Integer; var Key: string);
begin
  if IsAdminInstallMode then
  begin
    Root := HKEY_LOCAL_MACHINE;
    Key := '{#SysEnv}';
  end
  else
  begin
    Root := HKEY_CURRENT_USER;
    Key := 'Environment';
  end;
end;

function NeedsAddPath(Dir: string): boolean;
var
  Root: Integer;
  Key, Paths: string;
begin
  EnvKey(Root, Key);
  if not RegQueryStringValue(Root, Key, 'Path', Paths) then
  begin
    Result := True;
    exit;
  end;
  Result := Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(Paths) + ';') = 0;
end;

{ Take our folder back out of PATH on uninstall. }
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Root, P: Integer;
  Key, Paths, Dir: string;
begin
  if CurUninstallStep <> usPostUninstall then exit;
  EnvKey(Root, Key);
  if not RegQueryStringValue(Root, Key, 'Path', Paths) then exit;
  Dir := ExpandConstant('{app}');
  P := Pos(';' + Uppercase(Dir), Uppercase(Paths));
  if P > 0 then
  begin
    Delete(Paths, P, Length(Dir) + 1);
    RegWriteExpandStringValue(Root, Key, 'Path', Paths);
  end;
end;
