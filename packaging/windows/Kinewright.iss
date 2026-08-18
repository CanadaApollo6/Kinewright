#define AppName "Kinewright"
#define AppPublisher "Kinewright contributors"
#define AppUrl "https://github.com/CanadaApollo6/Kinewright"
#define AppExeName "Kinewright.exe"
#define AppVersion GetEnv("KINEWRIGHT_APP_VERSION")
#define NumericVersion GetEnv("KINEWRIGHT_NUMERIC_VERSION")
#define StageDir GetEnv("KINEWRIGHT_STAGE_DIR")
#define OutputDir GetEnv("KINEWRIGHT_OUTPUT_DIR")
#define RepoRoot GetEnv("KINEWRIGHT_REPO_ROOT")
#define AppIcon GetEnv("KINEWRIGHT_APP_ICON")

#if AppVersion == ""
  #error KINEWRIGHT_APP_VERSION is not set
#endif
#if NumericVersion == ""
  #error KINEWRIGHT_NUMERIC_VERSION is not set
#endif
#if StageDir == ""
  #error KINEWRIGHT_STAGE_DIR is not set
#endif
#if OutputDir == ""
  #error KINEWRIGHT_OUTPUT_DIR is not set
#endif
#if RepoRoot == ""
  #error KINEWRIGHT_REPO_ROOT is not set
#endif
#if AppIcon == ""
  #error KINEWRIGHT_APP_ICON is not set
#endif

[Setup]
AppId={{C90035A3-2E79-4C1F-BA95-09D6CA403D73}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppUrl}
AppSupportURL={#AppUrl}/issues
AppUpdatesURL={#AppUrl}/releases
VersionInfoVersion={#NumericVersion}.0
VersionInfoProductName={#AppName}
VersionInfoProductVersion={#AppVersion}
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableDirPage=no
DisableProgramGroupPage=no
AllowNoIcons=yes
LicenseFile={#RepoRoot}\LICENSE
OutputDir={#OutputDir}
OutputBaseFilename=Kinewright-{#AppVersion}-windows-x64-setup
SetupIconFile={#AppIcon}
UninstallDisplayName={#AppName}
UninstallDisplayIcon={app}\{#AppExeName}
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequired=lowest
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
RestartApplications=no

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop shortcut"; GroupDescription: "Additional shortcuts:"; Flags: unchecked

[Files]
Source: "{#StageDir}\{#AppExeName}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\*.dll"; DestDir: "{app}"; Flags: ignoreversion
; The FFmpeg CLI powers in-editor recording.
Source: "{#StageDir}\ffmpeg.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\LICENSES\*"; DestDir: "{app}\LICENSES"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExeName}"; WorkingDir: "{app}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExeName}"; WorkingDir: "{app}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExeName}"; Description: "Launch {#AppName}"; Flags: nowait postinstall skipifsilent
