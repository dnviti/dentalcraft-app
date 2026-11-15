; DentalCraft setup for Windows x64, built with makensis (NSIS 3) by
; packaging/windows/build-exe.sh, sourced by packaging/installers.sh.
; Per-user install, no administrator rights: %LOCALAPPDATA%\Programs\DentalCraft,
; Start menu shortcut, optional desktop shortcut, uninstaller, entry in Apps & features
; (HKCU). An update keeps the install folder and the desktop shortcut choice.
; Silent install, used by the in-app updater:  DentalCraft-<V>-setup.exe /S [/D=<folder>]
; It waits up to 30 s for a running dentalcraft.exe to exit, then fails (exit code 2).
; Defines: VERSION (0.2.0-alpha.4), WIN_VERSION (0.2.0.4), STAGE (the files), OUTFILE.

Unicode true
SetCompressor /SOLID lzma
ManifestDPIAware true
RequestExecutionLevel user

!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "x64.nsh"
!include "FileFunc.nsh"

!define APP_NAME "DentalCraft"
!define PUBLISHER "Daniele Viti"
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\com.dentalcraft.DentalCraft"

Name "${APP_NAME}"
OutFile "${OUTFILE}"
InstallDir "$LOCALAPPDATA\Programs\${APP_NAME}"
InstallDirRegKey HKCU "${UNINST_KEY}" "InstallLocation"
BrandingText "${APP_NAME} ${VERSION}"

!define MUI_ICON "${STAGE}/dentalcraft.ico"
!define MUI_UNICON "${STAGE}/dentalcraft.ico"
!define MUI_ABORTWARNING
!define MUI_COMPONENTSPAGE_NODESC
!define MUI_FINISHPAGE_RUN "$INSTDIR\dentalcraft.exe"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "${STAGE}/LICENSE.txt"
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
; The first language is the default; Windows picks Italian on an Italian system.
!insertmacro MUI_LANGUAGE "English"
!insertmacro MUI_LANGUAGE "Italian"

LangString NAME_DESKTOP ${LANG_ENGLISH} "Desktop shortcut"
LangString NAME_DESKTOP ${LANG_ITALIAN} "Collegamento sul desktop"
LangString MSG_RUNNING ${LANG_ENGLISH} "DentalCraft is still running. Close it, then choose Retry."
LangString MSG_RUNNING ${LANG_ITALIAN} "DentalCraft è ancora aperto. Chiudilo, poi scegli Riprova."
LangString MSG_X64 ${LANG_ENGLISH} "DentalCraft needs 64-bit Windows."
LangString MSG_X64 ${LANG_ITALIAN} "DentalCraft richiede Windows a 64 bit."

VIProductVersion "${WIN_VERSION}"
VIAddVersionKey /LANG=${LANG_ENGLISH} "ProductName" "${APP_NAME}"
VIAddVersionKey /LANG=${LANG_ENGLISH} "ProductVersion" "${VERSION}"
VIAddVersionKey /LANG=${LANG_ENGLISH} "FileVersion" "${VERSION}"
VIAddVersionKey /LANG=${LANG_ENGLISH} "FileDescription" "${APP_NAME} setup"
VIAddVersionKey /LANG=${LANG_ENGLISH} "CompanyName" "${PUBLISHER}"
VIAddVersionKey /LANG=${LANG_ENGLISH} "LegalCopyright" "Copyright (c) 2026 ${PUBLISHER}"

Section "${APP_NAME}" SecMain
  SectionIn RO
  SetOutPath "$INSTDIR"
  Call WaitForExit
  File "${STAGE}/dentalcraft.exe"
  File "${STAGE}/dentalcraft-cli.exe"
  File "${STAGE}/dentalcraft.ico"
  File "${STAGE}/README.md"
  File "${STAGE}/LICENSE.md"
  File "${STAGE}/THIRD-PARTY-NOTICES.md"
  File "${STAGE}/install-kind"
  RMDir /r "$INSTDIR\LICENSES"
  File /r "${STAGE}/LICENSES"
  WriteUninstaller "$INSTDIR\uninstall.exe"
  CreateShortcut "$SMPROGRAMS\${APP_NAME}.lnk" "$INSTDIR\dentalcraft.exe" "" "$INSTDIR\dentalcraft.ico" 0
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayName" "${APP_NAME}"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINST_KEY}" "Publisher" "${PUBLISHER}"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\dentalcraft.ico"
  WriteRegStr HKCU "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "${UNINST_KEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegStr HKCU "${UNINST_KEY}" "URLInfoAbout" "https://dentalcraft.app/"
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  WriteRegDWORD HKCU "${UNINST_KEY}" "EstimatedSize" $0
SectionEnd

Section /o "$(NAME_DESKTOP)" SecDesktop
  CreateShortcut "$DESKTOP\${APP_NAME}.lnk" "$INSTDIR\dentalcraft.exe" "" "$INSTDIR\dentalcraft.ico" 0
SectionEnd

Function .onInit
  ${IfNot} ${RunningX64}
    MessageBox MB_ICONSTOP "$(MSG_X64)" /SD IDOK
    Abort
  ${EndIf}
  ; An update keeps the desktop shortcut, if there is one.
  ${If} ${FileExists} "$DESKTOP\${APP_NAME}.lnk"
    SectionSetFlags ${SecDesktop} ${SF_SELECTED}
  ${EndIf}
FunctionEnd

; The in-app updater starts the setup and quits: wait until dentalcraft.exe can be
; replaced (a running executable cannot be opened for writing).
Function WaitForExit
  ${IfNot} ${FileExists} "$INSTDIR\dentalcraft.exe"
    Return
  ${EndIf}
  StrCpy $R0 0
  ${Do}
    ClearErrors
    FileOpen $R1 "$INSTDIR\dentalcraft.exe" a
    ${IfNot} ${Errors}
      FileClose $R1
      Return
    ${EndIf}
    IntOp $R0 $R0 + 1
    ${If} $R0 >= 60
      MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "$(MSG_RUNNING)" /SD IDCANCEL IDRETRY +2
      Abort "$(MSG_RUNNING)"
      StrCpy $R0 0
    ${EndIf}
    Sleep 500
  ${Loop}
FunctionEnd

Section "Uninstall"
  Delete "$SMPROGRAMS\${APP_NAME}.lnk"
  Delete "$DESKTOP\${APP_NAME}.lnk"
  Delete "$INSTDIR\dentalcraft.exe"
  Delete "$INSTDIR\dentalcraft-cli.exe"
  Delete "$INSTDIR\dentalcraft.ico"
  Delete "$INSTDIR\README.md"
  Delete "$INSTDIR\LICENSE.md"
  Delete "$INSTDIR\THIRD-PARTY-NOTICES.md"
  Delete "$INSTDIR\install-kind"
  RMDir /r "$INSTDIR\LICENSES"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  DeleteRegKey HKCU "${UNINST_KEY}"
SectionEnd
