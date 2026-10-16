; Setup for Klip, built with NSIS (Modern UI 2). scripts\stage.ps1 calls makensis with
;   /DVERSION=<x.y.z> /DSTAGE=<dist-stage folder> /DOUTFILE=<target\release\Klip-Setup.exe>
;
; Installs per user (no admin rights) into %LOCALAPPDATA%\Programs\Klip or a folder the
; user picks. Settings and clips live elsewhere and survive updates and uninstalling.
;
; Silent use, also by the automatic update:  Klip-Setup.exe /S /D=<folder>
; The /D= part must come last and carries no quotes.

Unicode true
ManifestDPIAware true
RequestExecutionLevel user
SetCompressor /SOLID lzma

!ifndef VERSION
  !define VERSION "0.0.0"
!endif
!ifndef STAGE
  !define STAGE "..\..\dist-stage"
!endif
!ifndef OUTFILE
  !define OUTFILE "Klip-Setup.exe"
!endif

!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\Klip"
!define RUN_KEY "Software\Microsoft\Windows\CurrentVersion\Run"
!define RECORDER "klip-recorder.exe"
!define APP "Klip.exe"

Name "Klip"
Caption "Klip ${VERSION}"
OutFile "${OUTFILE}"
InstallDir "$LOCALAPPDATA\Programs\Klip"
InstallDirRegKey HKCU "${UNINST_KEY}" "InstallLocation"
BrandingText "Klip ${VERSION}"

VIProductVersion "${VERSION}.0"
VIAddVersionKey /LANG=0 "ProductName" "Klip"
VIAddVersionKey /LANG=0 "FileDescription" "Klip Setup"
VIAddVersionKey /LANG=0 "FileVersion" "${VERSION}"
VIAddVersionKey /LANG=0 "ProductVersion" "${VERSION}"
VIAddVersionKey /LANG=0 "CompanyName" "Kirit"
VIAddVersionKey /LANG=0 "LegalCopyright" "Kirit"

!include "MUI2.nsh"
!include "nsDialogs.nsh"
!include "LogicLib.nsh"
!include "FileFunc.nsh"
!include "StrFunc.nsh"
${StrStr}
${UnStrStr}

; ---- look ----------------------------------------------------------------------------
!define MUI_ICON "klip.ico"
!define MUI_UNICON "klip.ico"
!define MUI_HEADERIMAGE
!define MUI_HEADERIMAGE_RIGHT
!define MUI_HEADERIMAGE_BITMAP "header.bmp"
!define MUI_HEADERIMAGE_BITMAP_STRETCH AspectFitHeight
!define MUI_WELCOMEFINISHPAGE_BITMAP "welcome.bmp"
!define MUI_WELCOMEFINISHPAGE_BITMAP_STRETCH AspectFitHeight
!define MUI_UNWELCOMEFINISHPAGE_BITMAP "welcome.bmp"
!define MUI_UNWELCOMEFINISHPAGE_BITMAP_STRETCH AspectFitHeight
!define MUI_ABORTWARNING

; ---- pages ---------------------------------------------------------------------------
!define MUI_WELCOMEPAGE_TITLE "$(WelcomeTitle)"
!define MUI_WELCOMEPAGE_TEXT "$(WelcomeText)"
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
Page custom QuickStartShow QuickStartLeave
!insertmacro MUI_PAGE_INSTFILES
!define MUI_FINISHPAGE_TITLE "$(FinishTitle)"
!define MUI_FINISHPAGE_TEXT "$(FinishText)"
!define MUI_FINISHPAGE_RUN
!define MUI_FINISHPAGE_RUN_TEXT "$(FinishRun)"
!define MUI_FINISHPAGE_RUN_FUNCTION OpenApp
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

; English first: it is used whenever Windows speaks neither language.
!insertmacro MUI_LANGUAGE "English"
!insertmacro MUI_LANGUAGE "German"

; ---- texts ---------------------------------------------------------------------------
LangString WelcomeTitle ${LANG_ENGLISH} "Welcome to Klip"
LangString WelcomeTitle ${LANG_GERMAN} "Willkommen bei Klip"
LangString WelcomeText ${LANG_ENGLISH} "Klip records your game in the background and saves the last minute when you press a key. Every audio source lands on its own track, so you decide later what is in the clip.$\r$\n$\r$\nKlip installs for your Windows account only and needs no admin rights. It never touches a game: no game memory, nothing loaded into a game process.$\r$\n$\r$\nClick Next to continue."
LangString WelcomeText ${LANG_GERMAN} "Klip nimmt dein Spiel im Hintergrund auf und speichert auf Tastendruck die letzte Minute. Jede Tonquelle landet auf einer eigenen Spur, also entscheidest du später, was im Clip ist.$\r$\n$\r$\nKlip wird nur für dein Windows-Konto installiert und braucht keine Adminrechte. Es fasst kein Spiel an: kein Spielspeicher, nichts wird in einen Spielprozess geladen.$\r$\n$\r$\nKlicke auf Weiter."
LangString QuickTitle ${LANG_ENGLISH} "Quick start"
LangString QuickTitle ${LANG_GERMAN} "Schnellstart"
LangString QuickSub ${LANG_ENGLISH} "Three things worth knowing, and two choices."
LangString QuickSub ${LANG_GERMAN} "Drei Dinge, die du wissen solltest, und zwei Entscheidungen."
LangString QuickKey1 ${LANG_ENGLISH} "Alt+F10  saves the last 60 seconds as a clip."
LangString QuickKey1 ${LANG_GERMAN} "Alt+F10  speichert die letzten 60 Sekunden als Clip."
LangString QuickKey2 ${LANG_ENGLISH} "Ctrl+Shift+F5  opens the overlay: clip, record, screenshot, pick the game."
LangString QuickKey2 ${LANG_GERMAN} "Strg+Umschalt+F5  öffnet das Overlay: Clip, Aufnahme, Screenshot, Spiel wählen."
LangString QuickKey3 ${LANG_ENGLISH} "Clips land in your Videos folder under Klip. The Klip app shows, cuts and exports them. Every key can be changed there."
LangString QuickKey3 ${LANG_GERMAN} "Clips landen in deinem Videos-Ordner unter Klip. Die Klip-App zeigt, schneidet und exportiert sie. Dort lässt sich jede Taste ändern."
LangString QuickAutostart ${LANG_ENGLISH} "Start Klip with Windows (recommended, otherwise nothing is buffered)"
LangString QuickAutostart ${LANG_GERMAN} "Klip mit Windows starten (empfohlen, sonst wird nichts gepuffert)"
LangString QuickDesktop ${LANG_ENGLISH} "Put a shortcut on the desktop"
LangString QuickDesktop ${LANG_GERMAN} "Verknüpfung auf dem Desktop anlegen"
LangString FinishTitle ${LANG_ENGLISH} "Klip is ready"
LangString FinishTitle ${LANG_GERMAN} "Klip ist bereit"
LangString FinishText ${LANG_ENGLISH} "The recorder now sits in the tray next to the clock and buffers. Press Alt+F10 after something good happened.$\r$\n$\r$\nKlip updates itself. Your settings and clips stay when you update or uninstall."
LangString FinishText ${LANG_GERMAN} "Der Recorder sitzt jetzt im Infobereich neben der Uhr und puffert. Drück Alt+F10, nachdem etwas Gutes passiert ist.$\r$\n$\r$\nKlip aktualisiert sich selbst. Einstellungen und Clips bleiben bei Updates und beim Entfernen erhalten."
LangString FinishRun ${LANG_ENGLISH} "Open Klip"
LangString FinishRun ${LANG_GERMAN} "Klip öffnen"
LangString AppOpen ${LANG_ENGLISH} "The Klip app is open. Close it and click Retry."
LangString AppOpen ${LANG_GERMAN} "Die Klip-App ist geöffnet. Schließe sie und klicke auf Wiederholen."

; ---- quick start page ----------------------------------------------------------------
Var WantAutostart
Var WantDesktop
Var AutostartBox
Var DesktopBox
; From the environment variable KLIP_SETUP_TEST: only copy and remove files. No running
; recorder is touched, no registry, no shortcuts. For tests next to a real installation.
Var Test

Function .onInit
  ReadEnvStr $Test KLIP_SETUP_TEST
  StrCpy $WantAutostart ${BST_CHECKED}
  StrCpy $WantDesktop ${BST_UNCHECKED}
FunctionEnd

Function QuickStartShow
  !insertmacro MUI_HEADER_TEXT "$(QuickTitle)" "$(QuickSub)"
  nsDialogs::Create 1018
  Pop $0
  ${If} $0 == error
    Abort
  ${EndIf}
  CreateFont $1 "Segoe UI" 10 700
  ${NSD_CreateLabel} 0 0 100% 18u "$(QuickKey1)"
  Pop $2
  SendMessage $2 ${WM_SETFONT} $1 0
  ${NSD_CreateLabel} 0 20u 100% 18u "$(QuickKey2)"
  Pop $2
  SendMessage $2 ${WM_SETFONT} $1 0
  ${NSD_CreateLabel} 0 42u 100% 26u "$(QuickKey3)"
  Pop $2
  ${NSD_CreateHLine} 0 76u 100% 1u ""
  Pop $2
  ${NSD_CreateCheckbox} 0 86u 100% 12u "$(QuickAutostart)"
  Pop $AutostartBox
  ${NSD_SetState} $AutostartBox $WantAutostart
  ${NSD_CreateCheckbox} 0 102u 100% 12u "$(QuickDesktop)"
  Pop $DesktopBox
  ${NSD_SetState} $DesktopBox $WantDesktop
  nsDialogs::Show
FunctionEnd

Function QuickStartLeave
  ${NSD_GetState} $AutostartBox $WantAutostart
  ${NSD_GetState} $DesktopBox $WantDesktop
FunctionEnd

Function OpenApp
  Exec '"$INSTDIR\${APP}"'
FunctionEnd

; ---- helpers -------------------------------------------------------------------------
; Asks a running recorder to leave, so its exe can be replaced. Older recorders do not know
; --quit, those are ended the hard way.
!macro StopRecorder
  ${If} $Test == ""
    ${If} ${FileExists} "$INSTDIR\${RECORDER}"
      nsExec::Exec '"$INSTDIR\${RECORDER}" --quit'
      Pop $0
    ${EndIf}
    nsExec::Exec 'taskkill /IM ${RECORDER} /F'
    Pop $0
    Sleep 500
  ${EndIf}
!macroend

; ---- install -------------------------------------------------------------------------
Section "Klip" SecMain
  SetShellVarContext current

  ; The app holds its files open. A silent update only starts once it is closed.
  ${IfNot} ${Silent}
    retry:
    nsExec::ExecToStack 'tasklist /FI "IMAGENAME eq ${APP}" /NH'
    Pop $0
    Pop $1
    ${StrStr} $2 $1 "${APP}"
    ${If} $2 != ""
      MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "$(AppOpen)" IDRETRY retry
      Abort
    ${EndIf}
  ${EndIf}

  !insertmacro StopRecorder

  SetOutPath "$INSTDIR"
  SetOverwrite on
  File "${STAGE}\${RECORDER}"
  File "${STAGE}\${APP}"
  SetOutPath "$INSTDIR\fonts"
  File /r "${STAGE}\fonts\*.*"
  SetOutPath "$INSTDIR"

  WriteUninstaller "$INSTDIR\uninstall.exe"
  ${If} $Test != ""
    Return
  ${EndIf}

  ; Start menu always. Desktop and autostart as chosen; a silent update leaves both as they are.
  CreateShortcut "$SMPROGRAMS\Klip.lnk" "$INSTDIR\${APP}" "" "$INSTDIR\${APP}" 0
  ${IfNot} ${Silent}
    ${If} $WantDesktop == ${BST_CHECKED}
      CreateShortcut "$DESKTOP\Klip.lnk" "$INSTDIR\${APP}" "" "$INSTDIR\${APP}" 0
    ${EndIf}
    ${If} $WantAutostart == ${BST_CHECKED}
      WriteRegStr HKCU "${RUN_KEY}" "Klip" '"$INSTDIR\${RECORDER}" --tray'
    ${Else}
      DeleteRegValue HKCU "${RUN_KEY}" "Klip"
    ${EndIf}
  ${EndIf}

  ; The entry under "Installed apps".
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayName" "Klip"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINST_KEY}" "Publisher" "Kirit"
  WriteRegStr HKCU "${UNINST_KEY}" "URLInfoAbout" "https://imkirit.dev/klip/"
  WriteRegStr HKCU "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\${APP}"
  WriteRegStr HKCU "${UNINST_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "${UNINST_KEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  IntFmt $0 "0x%08X" $0
  WriteRegDWORD HKCU "${UNINST_KEY}" "EstimatedSize" "$0"

  ; The recorder runs right away, in the tray.
  Exec '"$INSTDIR\${RECORDER}" --tray'
SectionEnd

; ---- uninstall -----------------------------------------------------------------------
Function un.onInit
  ReadEnvStr $Test KLIP_SETUP_TEST
FunctionEnd

Section "Uninstall"
  SetShellVarContext current
  ${If} $Test == ""
    !insertmacro StopRecorder
    nsExec::Exec 'taskkill /IM ${APP} /F'
    Pop $0

    ; Only remove the autostart entry if it points into this folder.
    ReadRegStr $0 HKCU "${RUN_KEY}" "Klip"
    ${UnStrStr} $1 $0 "$INSTDIR"
    ${If} $1 != ""
      DeleteRegValue HKCU "${RUN_KEY}" "Klip"
    ${EndIf}

    Delete "$SMPROGRAMS\Klip.lnk"
    Delete "$DESKTOP\Klip.lnk"
    DeleteRegKey HKCU "${UNINST_KEY}"
  ${EndIf}

  ; Only Klip's own files. The folder goes only if it is empty afterwards.
  Delete "$INSTDIR\${RECORDER}"
  Delete "$INSTDIR\${APP}"
  RMDir /r "$INSTDIR\fonts"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
SectionEnd
