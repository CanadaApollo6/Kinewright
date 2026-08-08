#define AppName "OpenReel"
#define AppPublisher "OpenReel contributors"
#define AppUrl "https://github.com/rielstamand/OpenReel"
#define AppExeName "OpenReel.exe"
#define AppVersion GetEnv("OPENREEL_APP_VERSION")
#define NumericVersion GetEnv("OPENREEL_NUMERIC_VERSION")
#define StageDir GetEnv("OPENREEL_STAGE_DIR")
#define OutputDir GetEnv("OPENREEL_OUTPUT_DIR")
#define RepoRoot GetEnv("OPENREEL_REPO_ROOT")
#define AppIcon GetEnv("OPENREEL_APP_ICON")

#if AppVersion == ""
  #error OPENREEL_APP_VERSION is not set
#endif
#if NumericVersion == ""
  #error OPENREEL_NUMERIC_VERSION is not set
#endif
#if StageDir == ""
  #error OPENREEL_STAGE_DIR is not set
#endif
#if OutputDir == ""
  #error OPENREEL_OUTPUT_DIR is not set
#endif
#if RepoRoot == ""
  #error OPENREEL_REPO_ROOT is not set
#endif
#if AppIcon == ""
  #error OPENREEL_APP_ICON is not set
#endif

[Setup]
AppId={{A366F620-9241-4CEB-9F09-3EB85B613348}
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
OutputBaseFilename=OpenReel-{#AppVersion}-windows-x64-setup
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
Source: "{#StageDir}\LICENSES\*"; DestDir: "{app}\LICENSES"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExeName}"; WorkingDir: "{app}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExeName}"; WorkingDir: "{app}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExeName}"; Description: "Launch {#AppName}"; Flags: nowait postinstall skipifsilent
