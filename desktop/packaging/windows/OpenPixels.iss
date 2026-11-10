; Inno Setup script for the Windows installer. scripts/package-windows.ps1 passes
; /DVersion, /DSource (the published folder) and /DOutput.
#ifndef Version
  #define Version "0.0.0"
#endif

[Setup]
AppId={{6E7B3E8A-0C1D-4C59-9C2E-5A0F2B7D4E31}
AppName=OpenPixels
AppVersion={#Version}
AppPublisher=OpenPixels contributors
AppPublisherURL=https://github.com/dollarkillerx/openpixels
DefaultDirName={autopf}\OpenPixels
DefaultGroupName=OpenPixels
DisableProgramGroupPage=yes
OutputDir={#Output}
OutputBaseFilename=OpenPixels-{#Version}-windows-x64-setup
SetupIconFile=..\..\OpenPixels.Desktop\Assets\app-icon.ico
UninstallDisplayIcon={app}\OpenPixels.exe
Compression=lzma2/max
SolidCompression=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequiredOverridesAllowed=dialog
WizardStyle=modern
LicenseFile=..\..\..\LICENSE

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "japanese"; MessagesFile: "compiler:Languages\Japanese.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#Source}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\OpenPixels"; Filename: "{app}\OpenPixels.exe"
Name: "{autodesktop}\OpenPixels"; Filename: "{app}\OpenPixels.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\OpenPixels.exe"; Description: "{cm:LaunchProgram,OpenPixels}"; Flags: nowait postinstall skipifsilent
