#ifndef AppVersion
  #define AppVersion "0.3.1"
#endif
#ifndef Payload
  #define Payload "..\dist\Connecty-x64"
#endif
#ifndef OutputDir
  #define OutputDir "..\dist"
#endif
#include "runtime.iss"

[Setup]
AppId={{C857E631-EF51-4D52-BCE6-9B9475A9CB9F}
AppName=Connecty
AppVersion={#AppVersion}
AppVerName=Connecty {#AppVersion}
AppPublisher=Chromatic
AppPublisherURL=https://chromatic.hu
AppSupportURL=https://github.com/Has-X/Connecty/issues
AppUpdatesURL=https://github.com/Has-X/Connecty/releases
VersionInfoCompany=Chromatic
VersionInfoDescription=Connecty Installer
VersionInfoProductName=Connecty
VersionInfoVersion={#AppVersion}
VersionInfoProductVersion={#AppVersion}
VersionInfoCopyright=Copyright (c) 2026 Chromatic
DefaultDirName={localappdata}\Programs\Chromatic\Connecty
DefaultGroupName=Connecty
PrivilegesRequired=lowest
ArchitecturesAllowed=x64os
ArchitecturesInstallIn64BitMode=x64os
SetupArchitecture=x64
MinVersion=10.0.22000
UninstallDisplayIcon={app}\Connecty.exe
UninstallDisplayName=Connecty
SetupIconFile=..\assets\Connecty.ico
OutputDir={#OutputDir}
OutputBaseFilename=Connecty-Setup-x64
WizardStyle=modern dynamic windows11 hidebevels
WizardSizePercent=110,110
WizardKeepAspectRatio=yes
WizardBackColor=#f4f8ff
WizardBackColorDynamicDark=#171b26
WizardBackImageFile=assets\background-light.png
WizardBackImageFileDynamicDark=assets\background-dark.png
WizardImageFile=
WizardSmallImageFile=
DisableWelcomePage=no
DisableDirPage=yes
DisableProgramGroupPage=yes
DisableReadyPage=yes
DisableStartupPrompt=yes
ShowLanguageDialog=no
Compression=lzma2/ultra64
SolidCompression=yes
CloseApplications=yes
RestartApplications=no
Uninstallable=yes
ChangesAssociations=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "hungarian"; MessagesFile: "compiler:Languages\Hungarian.isl"

[Messages]
WelcomeLabel1=Connecty
WelcomeLabel2=Keep your SSH tunnels connected.
FinishedHeadingLabel=Connecty is ready.
FinishedLabel=Open Connecty to manage your tunnels.

[Files]
Source: "{#Payload}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{userprograms}\Connecty"; Filename: "{app}\Connecty.exe"; WorkingDir: "{app}"; AppUserModelID: "Chromatic.Connecty"

[Run]
Filename: "{app}\Connecty.exe"; Description: "{cm:LaunchProgram,Connecty}"; Flags: nowait postinstall skipifsilent

[Code]
var
  Prepared, Finished: Boolean;
  DownloadPage: TDownloadWizardPage;

function Maintenance(const Exe, Operation: String): Boolean;
var Code: Integer;
begin
  Result := Exec(Exe, Operation + ' "' + ExpandConstant('{tmp}\connecty-state.json') + '"', '', SW_HIDE, ewWaitUntilTerminated, Code) and (Code = 0);
end;

procedure InitializeWizard;
var Logo: TBitmapImage;
begin
  DownloadPage := CreateDownloadPage('Windows components', 'Preparing the shared Windows UI runtime.', nil);
  WizardForm.WizardBitmapImage.Visible := False;
  WizardForm.WizardBitmapImage2.Visible := False;
  WizardForm.WizardSmallBitmapImage.Visible := False;
  ExtractTemporaryFile('Connecty.png');
  Logo := TBitmapImage.Create(WizardForm);
  Logo.Parent := WizardForm.WelcomePage;
  Logo.PngImage.LoadFromFile(ExpandConstant('{tmp}\Connecty.png'));
  Logo.SetBounds(ScaleX(28), ScaleY(28), ScaleX(72), ScaleY(72));
  Logo.Stretch := True;
  Logo.BackColor := clNone;
  WizardForm.WelcomeLabel2.Caption := 'Keep your SSH tunnels connected.';
  WizardForm.WelcomeLabel1.Left := ScaleX(28);
  WizardForm.WelcomeLabel1.Top := ScaleY(124);
  WizardForm.WelcomeLabel1.Width := ScaleX(380);
  WizardForm.WelcomeLabel1.Font.Size := 24;
  WizardForm.WelcomeLabel2.Left := ScaleX(28);
  WizardForm.WelcomeLabel2.Top := ScaleY(174);
  WizardForm.WelcomeLabel2.Width := ScaleX(380);
  WizardForm.FinishedHeadingLabel.Left := ScaleX(28);
  WizardForm.FinishedHeadingLabel.Top := ScaleY(60);
  WizardForm.FinishedHeadingLabel.Width := ScaleX(380);
  WizardForm.FinishedLabel.Left := ScaleX(28);
  WizardForm.FinishedLabel.Top := ScaleY(114);
  WizardForm.FinishedLabel.Width := ScaleX(380);
  WizardForm.RunList.Left := ScaleX(28);
  WizardForm.RunList.Top := ScaleY(190);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
var Code: Integer;
begin
  Result := '';
  if Prepared then exit;
  ExtractTemporaryFile('Connecty.exe');
  ExtractTemporaryFile('Connecty.Agent.exe');
  ExtractTemporaryFile('Microsoft.WindowsAppRuntime.Bootstrap.dll');
  if not Exec(ExpandConstant('{tmp}\Connecty.exe'), '--runtime-check', '', SW_HIDE, ewWaitUntilTerminated, Code) or (Code <> 0) then begin
    DownloadPage.Clear;
    DownloadPage.Add('{#RuntimeUrl}', 'WindowsAppRuntimeInstall-x64.exe', '{#RuntimeSha256}');
    DownloadPage.Show;
    try
      try
        DownloadPage.Download;
      except
        Result := 'The Windows UI runtime could not be downloaded. Check your connection and retry.';
        exit;
      end;
    finally
      DownloadPage.Hide;
    end;
    if not Exec(ExpandConstant('{tmp}\WindowsAppRuntimeInstall-x64.exe'), '--quiet', '', SW_HIDE, ewWaitUntilTerminated, Code) or ((Code <> 0) and (Code <> 3010)) then begin
      Result := 'The Windows UI runtime could not be installed.';
      exit;
    end;
    if not Exec(ExpandConstant('{tmp}\Connecty.exe'), '--runtime-check', '', SW_HIDE, ewWaitUntilTerminated, Code) or (Code <> 0) then begin
      Result := 'The Windows UI runtime is not ready. Restart Windows and retry.';
      exit;
    end;
  end;
  Prepared := Maintenance(ExpandConstant('{tmp}\Connecty.Agent.exe'), '--setup-prepare');
  if not Prepared then Result := 'Connecty could not close its previous connection safely. Close other copies and retry.';
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then begin
    Finished := Maintenance(ExpandConstant('{app}\Connecty.Agent.exe'), '--setup-finish');
    if not Finished then RaiseException('The files were installed, but connection recovery failed. Open Connecty to retry.');
  end;
end;

procedure DeinitializeSetup;
begin
  if Prepared and not Finished then
    Maintenance(ExpandConstant('{tmp}\Connecty.Agent.exe'), '--setup-rollback');
end;

function InitializeUninstall: Boolean;
var Code: Integer;
begin
  Result := Exec(ExpandConstant('{app}\Connecty.Agent.exe'), '--setup-remove', '', SW_HIDE, ewWaitUntilTerminated, Code) and (Code = 0);
  if not Result and not UninstallSilent then
    MsgBox('Connecty could not stop safely. Close it and retry uninstall.', mbError, MB_OK);
end;
