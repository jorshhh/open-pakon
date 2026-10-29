; Pakon for Windows: a per-user install (no administrator rights) to
; %LOCALAPPDATA%\Programs\Pakon (fixed, so the uninstaller can remove the
; whole folder), with Start menu and desktop shortcuts and an
; entry in Settings > Apps. The scanner's USB driver is set up once by
; driver\pakon_driver.exe, which asks for administrator rights. Built by build.sh:
;   makensis -DVERSION=0.7.0 -DSTAGE=<staged app> -DOUTFILE=<setup.exe> installer.nsi
Unicode true
SetCompressor /SOLID lzma
!include "MUI2.nsh"

!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\Pakon"

Name "Pakon"
OutFile "${OUTFILE}"
InstallDir "$LOCALAPPDATA\Programs\Pakon"
InstallDirRegKey HKCU "Software\Pakon" "InstallDir"
RequestExecutionLevel user
VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "Pakon"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "FileDescription" "Pakon installer"
VIAddVersionKey "LegalCopyright" "See LICENSE and NOTICE"

!define MUI_ICON "${STAGE}\pakon.ico"
!define MUI_UNICON "${STAGE}\pakon.ico"
!define MUI_FINISHPAGE_RUN "$INSTDIR\Pakon.exe"
!insertmacro MUI_PAGE_LICENSE "${STAGE}\LICENSE"
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Section "Pakon"
  SetOutPath "$INSTDIR"
  ; an update replaces the app; settings and scans are elsewhere
  RMDir /r "$INSTDIR\Lib"
  RMDir /r "$INSTDIR\web"
  RMDir /r "$INSTDIR\tools"
  File /r "${STAGE}\*"
  WriteUninstaller "$INSTDIR\Uninstall.exe"

  ; the scanner's USB driver, once: an update finds the marker it leaves
  ; (pakon_driver.c) and skips this
  IfFileExists "$INSTDIR\driver\installed-1" driver_done
    DetailPrint "Setting up the scanner's USB driver (Windows asks for an administrator)"
    ExecShellWait "open" '"$INSTDIR\driver\pakon_driver.exe"' "install --quiet"
  driver_done:

  CreateShortcut "$SMPROGRAMS\Pakon.lnk" "$INSTDIR\Pakon.exe"
  CreateShortcut "$DESKTOP\Pakon.lnk" "$INSTDIR\Pakon.exe"

  WriteRegStr HKCU "Software\Pakon" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayName" "Pakon"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\Pakon.exe"
  WriteRegStr HKCU "${UNINST_KEY}" "Publisher" "Pakon"
  WriteRegStr HKCU "${UNINST_KEY}" "URLInfoAbout" "https://github.com/jorshhh/open-pakon"
  WriteRegStr HKCU "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_KEY}" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1
SectionEnd

Section "Uninstall"
  ; the app only: settings, EEPROM archives and logs in %LOCALAPPDATA%\Pakon
  ; and the scans in the work folder stay
  IfFileExists "$INSTDIR\driver\installed-1" 0 driver_kept
    MessageBox MB_YESNO|MB_ICONQUESTION "Also remove the scanner's USB driver? Keep it if you will reinstall Pakon." /SD IDNO IDNO driver_kept
    ExecShellWait "open" '"$INSTDIR\driver\pakon_driver.exe"' "uninstall --quiet"
  driver_kept:
  Delete "$SMPROGRAMS\Pakon.lnk"
  Delete "$DESKTOP\Pakon.lnk"
  RMDir /r "$INSTDIR"
  DeleteRegKey HKCU "${UNINST_KEY}"
  DeleteRegKey HKCU "Software\Pakon"
SectionEnd
