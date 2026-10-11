; Per-user Inno Setup installer for the reference application.
;
; Built by tools/e2e/windows/run-e2e.ps1 with ISCC (Inno Setup 6.3 or newer):
;
;   ISCC.exe /DAppVersion=1.1.0 /DNumericVersion=1.1.0.0 /DFeedArch=x86_64 \
;     /DInnoArch=x64compatible /DSourceExe=<reference-app.exe> /DOutputDir=<dir> \
;     [/DE2EReport=<report file>] reference-app.iss
;
; The settings are the required ones from docs/windows-installers.md: a
; fixed AppId, a per-user install that never elevates, UsePreviousAppDir, a
; SetupMutex, Restart Manager closing, and a [Run] entry without
; skipifsilent so a silent update relaunches the app.
;
; With E2EReport defined, the installer also appends to the app's
; end-to-end report. Before Restart Manager runs, it gives the old version
; up to a minute to quit on its own and records whether it did, so a test
; can tell an app that quit after handing off from one Setup had to close.

#ifndef AppVersion
  #error Define AppVersion, for example /DAppVersion=1.1.0
#endif
#ifndef NumericVersion
  #error Define NumericVersion, for example /DNumericVersion=1.1.0.0
#endif
#ifndef FeedArch
  #error Define FeedArch: x86_64 or aarch64
#endif
#ifndef InnoArch
  #error Define InnoArch: x64compatible or arm64
#endif
#ifndef SourceExe
  #error Define SourceExe, the reference-app.exe to package
#endif
#ifndef OutputDir
  #define OutputDir "."
#endif

[Setup]
AppId={{6F1D2C1E-3B57-4C4F-9C3E-2A8D7B0E5F41}
AppName=GPUI Auto Update Reference App
AppVersion={#AppVersion}
AppPublisher=gpui-auto-update
AppPublisherURL=https://github.com/thedavidweng/gpui-auto-update
DefaultDirName={autopf}\GPUI Auto Update Reference App
DisableProgramGroupPage=yes
DisableDirPage=auto
UsePreviousAppDir=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=commandline
SetupMutex=GpuiAutoUpdateReferenceAppSetup
CloseApplications=force
RestartApplications=no
ArchitecturesAllowed={#InnoArch}
ArchitecturesInstallIn64BitMode={#InnoArch}
OutputDir={#OutputDir}
OutputBaseFilename=reference-app-{#AppVersion}-windows-{#FeedArch}-setup
VersionInfoVersion={#NumericVersion}
VersionInfoProductVersion={#NumericVersion}
VersionInfoProductTextVersion={#AppVersion}
VersionInfoTextVersion={#AppVersion}
Compression=lzma2/fast
SolidCompression=no
WizardStyle=modern

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; DestName: "reference-app.exe"; Flags: ignoreversion

[Run]
; `/NORUN=1` lets a test install without launching the app.
Filename: "{app}\reference-app.exe"; Flags: nowait postinstall; Check: ShouldLaunch

[Code]
function ShouldLaunch: Boolean;
begin
  Result := ExpandConstant('{param:NORUN|0}') <> '1';
end;

#ifdef E2EReport
// Whether a process still has the executable open: Windows refuses to open
// a running image for exclusive writing.
function InUse(const Path: String): Boolean;
var
  Stream: TFileStream;
begin
  Result := False;
  if not FileExists(Path) then
    Exit;
  try
    Stream := TFileStream.Create(Path, fmOpenReadWrite or fmShareExclusive);
    Stream.Free;
  except
    Result := True;
  end;
end;

procedure Report(const Line: String);
begin
  SaveStringToFile('{#E2EReport}', Line + #13#10, True);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Exe, Running: String;
  Waited: Integer;
begin
  Exe := ExpandConstant('{app}\reference-app.exe');
  Waited := 0;
  while InUse(Exe) and (Waited < 60000) do
  begin
    Sleep(250);
    Waited := Waited + 250;
  end;
  if InUse(Exe) then
    Running := 'yes'
  else
    Running := 'no';
  Report('installer-replacing-files app-running=' + Running + ' waited-ms=' + IntToStr(Waited) + ' dir=' + ExpandConstant('{app}'));
  Result := '';
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
    Report('installer-done {#AppVersion}');
end;
#endif
