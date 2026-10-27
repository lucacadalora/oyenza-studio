; Oyenza Studio installer (Inno Setup 6.5.2+, for the PNG wizard images).
;
; Releases ship installer/poni.nsi (NSIS), which 0.1.x installs came from; CI builds this one too as a check.
;
; Build after `cargo build -p poni --release`:
;   iscc installer\poni.iss                      -> dist\Oyenza-Studio-Setup-0.2.13.exe
;   iscc /DAppVersion=0.2.0 installer\poni.iss   (another version)
;
; Per-user install: no admin rights and no UAC prompt. The app goes to
; %LOCALAPPDATA%\Programs\Poni; settings and data stay in %APPDATA%\Poni and
; survive uninstalling (only the cache in %LOCALAPPDATA%\Poni is removed).

#ifndef AppVersion
  #define AppVersion "0.2.13"
#endif
#ifndef ExeSource
  #define ExeSource "..\target\release\poni.exe"
#endif
#define AppName "Oyenza Studio"
; Folder, data and Run value names stay "Poni" (the app and 0.1.x installs use them).
#define AppDir "Poni"
#define AppPublisher "Jatevo Labs"
#define AppExe "poni.exe"
; The island window's class (crates/poni/src/window.rs), used to ask a running copy to quit.
#define IslandClass "Poni.Island"

[Setup]
; Never change the AppId: upgrades and the uninstaller find the install by it.
AppId={{6F1D2B7A-3C84-4E59-A1D6-8B2E5F0C9A47}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppPublisher}
AppCopyright=Copyright (C) 2026 {#AppPublisher}
VersionInfoVersion={#AppVersion}
VersionInfoCompany={#AppPublisher}
VersionInfoDescription={#AppName} Setup
VersionInfoProductName={#AppName}
PrivilegesRequired=lowest
DefaultDirName={localappdata}\Programs\{#AppDir}
DisableDirPage=yes
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir=..\dist
OutputBaseFilename=Oyenza-Studio-Setup-{#AppVersion}
SetupIconFile=..\crates\poni\res\poni.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
WizardStyle=modern
; Oyen hanging from the island on the finish page, Poni's logo in the header (both from tools/make_brand.py).
; Setup picks the file closest to its image area: 100%, 150% and 200% scaling.
WizardImageFile=art\wizard-202x386.png,art\wizard-336x643.png,art\wizard-430x824.png
WizardSmallImageFile=art\wizard-small-58.png,art\wizard-small-77.png,art\wizard-small-97.png,art\wizard-small-116.png,art\wizard-small-124.png,art\wizard-small-143.png,art\wizard-small-159.png
Compression=lzma2/max
SolidCompression=yes
; A running copy is asked to quit in PrepareToInstall; Restart Manager is the safety net.
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "autostart"; Description: "Start with Windows"; GroupDescription: "Startup:"

[Files]
Source: "{#ExeSource}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\THIRD-PARTY-NOTICES.txt"; DestDir: "{app}"; Flags: ignoreversion

[InstallDelete]
; The shortcut the 0.1.x versions from 0.1.2 on made, before the app was renamed Oyenza Studio.
Type: files; Name: "{autoprograms}\Oyen Poni.lnk"

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"; Comment: "A living notch for Windows"

[Registry]
; Same value the app writes from its "Start with Windows" menu item (autostart.rs): the quoted exe path.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "{#AppDir}"; ValueData: """{app}\{#AppExe}"""; Flags: uninsdeletevalue; Tasks: autostart

[Run]
Filename: "{app}\{#AppExe}"; Description: "Launch {#AppName}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
; Make sure no copy is still running before its files are removed.
Filename: "{sys}\taskkill.exe"; Parameters: "/IM {#AppExe} /F"; Flags: runhidden waituntilterminated; RunOnceId: "StopPoni"

[UninstallDelete]
; Cache and log only (lyrics, poni.log); settings and data in %APPDATA%\Poni are kept.
Type: filesandordirs; Name: "{localappdata}\{#AppDir}"

[Code]
const
  RunKey = 'Software\Microsoft\Windows\CurrentVersion\Run';
  WM_CLOSE = $0010;

{ Ask a running Poni to quit (it saves its data on the way out); force it if it doesn't within 3 s. }
procedure StopPoni();
var
  I, ResultCode: Integer;
begin
  if FindWindowByClassName('{#IslandClass}') = 0 then
    Exit;
  PostMessage(FindWindowByClassName('{#IslandClass}'), WM_CLOSE, 0, 0);
  for I := 1 to 30 do
  begin
    if FindWindowByClassName('{#IslandClass}') = 0 then
      Exit;
    Sleep(100);
  end;
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/IM {#AppExe} /F', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  StopPoni();
  Result := '';
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  { Unticking "Start with Windows" on a reinstall turns it off, even if the app's menu turned it on. }
  if (CurStep = ssPostInstall) and not WizardIsTaskSelected('autostart') then
    RegDeleteValue(HKEY_CURRENT_USER, RunKey, '{#AppDir}');
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  { Runs before [UninstallRun], so a running copy gets the chance to save and quit cleanly. }
  if CurUninstallStep = usUninstall then
    StopPoni();
  { The app may have written the Run value itself; never leave it pointing at a removed exe. }
  if CurUninstallStep = usPostUninstall then
    RegDeleteValue(HKEY_CURRENT_USER, RunKey, '{#AppDir}');
end;
