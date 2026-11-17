; Pakon for Windows: a per-user install (no administrator rights) to
; %LOCALAPPDATA%\Programs\Pakon (fixed, so the uninstaller can remove the
; whole folder), with Start menu and desktop shortcuts and an
; entry in Settings > Apps. The scanner's USB driver is set up once by
; driver\pakon_driver.exe, which asks for administrator rights. Built by build.sh:
;   makensis -DVERSION=0.7.0 -DSTAGE=<staged app> -DOUTFILE=<setup.exe> installer.nsi
;
; In-place updates (WinSparkle, pakon-app winsparkle.rs) run it as
;   Pakon-Setup.exe /S /UPDATE
; silent, it waits for the running Pakon.exe to quit (WinSparkle asks it to
; right after starting the installer), installs, and starts the new one.
Unicode true
SetCompressor /SOLID lzma
!include "MUI2.nsh"
!include "FileFunc.nsh"

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

Var Update

; /UPDATE: wait (at most 2 minutes: the scanner's teardown) until the old
; Pakon.exe can be replaced, i.e. it has quit
Function .onInit
  ${GetParameters} $0
  ClearErrors
  ${GetOptions} $0 "/UPDATE" $1
  IfErrors init_done
  StrCpy $Update 1
  StrCpy $2 0
  wait_quit:
    IfFileExists "$INSTDIR\Pakon.exe" 0 init_done
    ClearErrors
    FileOpen $3 "$INSTDIR\Pakon.exe" a
    IfErrors 0 quit_done
    IntOp $2 $2 + 1
    IntCmp $2 240 init_done
    Sleep 500
    Goto wait_quit
  quit_done:
    FileClose $3
  init_done:
FunctionEnd

Section "Pakon"
  SetOutPath "$INSTDIR"
  ; an update replaces the app; settings and scans are elsewhere. Also
  ; removes what the earlier Python app installed: the embedded
  ; Python, its libraries, the C tools and the Python sources
  RMDir /r "$INSTDIR\Lib"
  RMDir /r "$INSTDIR\web"
  RMDir /r "$INSTDIR\tools"
  RMDir /r "$INSTDIR\build"
  RMDir /r "$INSTDIR\packaging"
  Delete "$INSTDIR\python*.*"
  Delete "$INSTDIR\*.pyd"
  Delete "$INSTDIR\*.dll"
  Delete "$INSTDIR\LICENSE.txt"
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

  ; an in-place update: back to the app (the Finish page's Run otherwise),
  ; started by Explorer as from the Start menu. Exec would give it this
  ; installer's environment, inherited from the old Pakon.exe: the old
  ; release.env's PAKON_VERSION and the rest, which the app keeps over its
  ; own release.env, so the new app would report the old version
  StrCmp $Update 1 0 +2
    Exec '"$WINDIR\explorer.exe" "$INSTDIR\Pakon.exe"'
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
