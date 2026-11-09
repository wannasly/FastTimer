#ifndef AppVersion
  #error AppVersion must be passed by the packaging script
#endif
#define RepoURL "https://github.com/wannasly/FastTimer"

[Setup]
AppId=FastTimer.wannasly
AppName=FastTimer
AppVersion={#AppVersion}
AppPublisher=wannasly
AppPublisherURL={#RepoURL}
AppSupportURL={#RepoURL}/issues
AppUpdatesURL={#RepoURL}/releases
AppMutex=Local\FastTimer.Desktop.0
DefaultDirName={localappdata}\Programs\FastTimer
DefaultGroupName=FastTimer
DisableProgramGroupPage=yes
AllowNoIcons=yes
PrivilegesRequired=lowest
MinVersion=10.0.22000
ArchitecturesAllowed=x64os
ArchitecturesInstallIn64BitMode=x64os
CreateUninstallRegKey=no
CloseApplications=no
WizardStyle=modern
SetupIconFile=..\assets\fasttimer.ico
OutputBaseFilename=FastTimer-{#AppVersion}-windows-x64-setup
Compression=lzma2
SolidCompression=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "russian"; MessagesFile: "compiler:Languages\Russian.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; Flags: unchecked

[Files]
Source: "{#BinDir}\fasttimer.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#BinDir}\fasttimer-cli.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\RESEARCH.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\FastTimer"; Filename: "{app}\fasttimer.exe"; WorkingDir: "{app}"
Name: "{group}\Uninstall FastTimer"; Filename: "{uninstallexe}"
Name: "{userdesktop}\FastTimer"; Filename: "{app}\fasttimer.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\fasttimer.exe"; Description: "{cm:LaunchProgram,FastTimer}"; Flags: nowait postinstall skipifsilent

[Code]
procedure CurUninstallStepChanged(Step: TUninstallStep);
var
  Path: String;
  Shell, Link: Variant;
begin
  if Step <> usUninstall then
    Exit;
  Path := ExpandConstant('{userstartup}\FastTimer.lnk');
  if not FileExists(Path) then
    Exit;
  try
    Shell := CreateOleObject('WScript.Shell');
    Link := Shell.CreateShortcut(Path);
    if CompareText(Link.TargetPath, ExpandConstant('{app}\fasttimer.exe')) = 0 then
      DeleteFile(Path);
  except
    Log('Could not inspect the FastTimer startup shortcut.');
  end;
end;
