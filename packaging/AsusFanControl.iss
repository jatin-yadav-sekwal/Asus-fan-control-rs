; Asus Fan Control - Inno Setup installer
;
; Prerequisites: package.ps1 has already staged dist\AsusFanControl-rs-v<ver>
; and ISCC.exe is on PATH (winget/choco both ship Inno Setup 6).
;
; Build from anywhere:
;   iscc /DAppVersion=0.1.1 packaging\AsusFanControl.iss
;
; What this must deliver, beyond "copy the files":
;   * a Start Menu entry so the app shows up in the app list / search,
;   * an entry under Settings > Apps with a working uninstaller,
;   * an optional "start with Windows" task created through the very same
;     asus-driver-cli autostart code the GUI uses, so there is only one
;     definition of the task name, schedule and privilege level.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

#define StagingDir "..\dist\AsusFanControl-rs-v" + AppVersion
#define AppExe "{app}\bin\asus-fan-app.exe"
#define CliExe "{app}\bin\asus-driver-cli.exe"

[Setup]
; Fixed AppId: upgrades must overwrite the same uninstall key, otherwise
; every install would leave a stale "Asus Fan Control" entry behind.
AppId={{B4C53D7A-1F62-4C89-9E3B-6A8D20F71E45}
AppName=Asus Fan Control
AppVersion={#AppVersion}
AppVerName=Asus Fan Control {#AppVersion}
AppPublisher=Asus Fan Control contributors
AppPublisherURL=https://github.com/jatin-yadav-sekwal/Asus-fan-control-rs
AppSupportURL=https://github.com/jatin-yadav-sekwal/Asus-fan-control-rs/issues
DefaultDirName={localappdata}\Programs\AsusFanControl
DisableProgramGroupPage=yes
; The optional startup task needs /RL HIGHEST, which a filtered token cannot
; create, so the installer itself runs elevated.
PrivilegesRequired=admin
; Deliberate: the app writes app.log/panic.log next to its own executable, and
; the SYSTEM helper must append to that same file, so the install folder has to
; stay writable by the pre-elevation process. Program Files would break that.
UsedUserAreasWarning=no
OutputDir=..\dist
OutputBaseFilename=AsusFanControl-rs-v{#AppVersion}-Setup
SetupIconFile=..\asus-fan-control-rs\assets\app.ico
UninstallDisplayIcon={#AppExe}
UninstallDisplayName=Asus Fan Control
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
MinVersion=10.0
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
VersionInfoVersion={#AppVersion}
VersionInfoProductTextVersion={#AppVersion}
; Stop the running app before overwriting it. The helper restores BIOS fan
; control and deletes its own task when the GUI closes, so a force-close here
; is still a clean shutdown.
AppMutex=Global\AsusFanControlSingleInstance
CloseApplications=force
SetupLogging=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "startup"; Description: "Start with Windows (runs elevated, no UAC prompt at logon)"; GroupDescription: "Additional options:"; Flags: unchecked
Name: "desktopicon"; Description: "Create a &desktop shortcut"; GroupDescription: "Additional options:"; Flags: unchecked

[Files]
Source: "{#StagingDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\Asus Fan Control"; Filename: "{#AppExe}"; Comment: "Control the laptop fan curve"
Name: "{autodesktop}\Asus Fan Control"; Filename: "{#AppExe}"; Tasks: desktopicon

[Run]
Filename: "{#CliExe}"; Parameters: "autostart"; StatusMsg: "Registering start with Windows..."; Tasks: startup; Flags: runhidden
Filename: "{#AppExe}"; Description: "&Launch Asus Fan Control"; Flags: nowait postinstall skipifsilent unchecked

[UninstallRun]
; Remove the logon task first, while the binaries are still on disk - the
; task points at {app}\bin\asus-fan-app.exe and would otherwise survive the
; uninstall as a broken entry.
Filename: "{#CliExe}"; Parameters: "autostart --disable"; RunOnceId: "DisableAutostart"; Flags: runhidden
Filename: "{sys}\schtasks.exe"; Parameters: "/Delete /TN AsusFanControlSystemHelper /F"; RunOnceId: "DeleteHelperTask"; Flags: runhidden
