; Installer für den Launcher (pro Benutzer, ohne Admin-Rechte).
;
;   makensis /DVERSION=0.2.0 /DSRC=..\target\release /DOUTFILE=..\dist\MyLauncher_0.2.0_x64-setup.exe mylauncher.nsi
;
; Mit /DTHUMBPRINT=<SHA1> signiert makensis Installer und Uninstaller selbst (signtool muss im PATH liegen).
; Installiert wird nach %LOCALAPPDATA%\MyLauncher – derselbe Pfad, den die Prism-Hooks eingetragen haben.

Unicode true
!include "MUI2.nsh"

!define PRODUCT "Birkenfeld's Minecraft Launcher"
!define PUBLISHER "Stephanides IT e.U."
!define EXE "mylauncher.exe"
!define UNINSTKEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\MyLauncher"

!ifndef VERSION
  !error "VERSION fehlt (/DVERSION=x.y.z)"
!endif
!ifndef SRC
  !define SRC "..\target\release"
!endif
!ifndef OUTFILE
  !define OUTFILE "MyLauncher_${VERSION}_x64-setup.exe"
!endif

!ifdef THUMBPRINT
  !define SIGN `signtool sign /sha1 ${THUMBPRINT} /fd sha256 /tr http://time.certum.pl /td sha256 /d "${PRODUCT}" "%1"`
  !finalize `${SIGN}` = 0
  !uninstfinalize `${SIGN}` = 0
!endif

Name "${PRODUCT}"
OutFile "${OUTFILE}"
InstallDir "$LOCALAPPDATA\MyLauncher"
RequestExecutionLevel user
SetCompressor /SOLID lzma
BrandingText "${PRODUCT} ${VERSION}"

VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "${PRODUCT}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "FileDescription" "${PRODUCT} Setup"
VIAddVersionKey "CompanyName" "${PUBLISHER}"
VIAddVersionKey "LegalCopyright" "${PUBLISHER}"

!define MUI_ICON "..\app\icons\icon.ico"
!define MUI_UNICON "..\app\icons\icon.ico"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\${EXE}"
!define MUI_FINISHPAGE_RUN_TEXT "${PRODUCT} starten"

!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "German"

Section "Install"
  SetOutPath "$INSTDIR"

  ; Beim Selbst-Update beendet sich der Launcher erst kurz nach dem Start des Installers –
  ; so lange warten, bis die alte Exe freigegeben ist.
  StrCpy $0 0
  wait:
    ClearErrors
    IfFileExists "$INSTDIR\${EXE}" 0 ready
    Delete "$INSTDIR\${EXE}"
    IfErrors 0 ready
    IntOp $0 $0 + 1
    IntCmp $0 20 locked
    Sleep 500
    Goto wait
  locked:
    MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "${PRODUCT} läuft noch. Bitte schließen und erneut versuchen." IDRETRY wait
    Abort
  ready:

  File "${SRC}\${EXE}"
  WriteUninstaller "$INSTDIR\uninstall.exe"

  CreateShortcut "$SMPROGRAMS\${PRODUCT}.lnk" "$INSTDIR\${EXE}"
  CreateShortcut "$DESKTOP\${PRODUCT}.lnk" "$INSTDIR\${EXE}"

  WriteRegStr HKCU "${UNINSTKEY}" "DisplayName" "${PRODUCT}"
  WriteRegStr HKCU "${UNINSTKEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINSTKEY}" "Publisher" "${PUBLISHER}"
  WriteRegStr HKCU "${UNINSTKEY}" "DisplayIcon" "$INSTDIR\${EXE}"
  WriteRegStr HKCU "${UNINSTKEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTKEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegDWORD HKCU "${UNINSTKEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTKEY}" "NoRepair" 1
SectionEnd

; Einstellungen in %APPDATA%\MyLauncher bleiben erhalten.
Section "Uninstall"
  Delete "$INSTDIR\${EXE}"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\${PRODUCT}.lnk"
  Delete "$DESKTOP\${PRODUCT}.lnk"
  DeleteRegKey HKCU "${UNINSTKEY}"
SectionEnd
