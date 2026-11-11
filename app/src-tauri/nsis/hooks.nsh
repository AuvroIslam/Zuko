; Uninstall hooks for the NSIS installer.
;
; The app stages zuko-hook.exe into %LOCALAPPDATA%\Zuko\bin at launch, so the
; installer never recorded it and the default uninstaller leaves it behind. The
; inbox and the log live in the same place and are ours too.
;
; Claude Code's own settings.json is deliberately NOT edited here: it belongs to
; the user, it may contain hooks from other tools, and rewriting somebody's
; config from an uninstaller with no diff and no consent is exactly what the rest
; of this app goes out of its way not to do. A leftover hook entry is harmless (a
; missing relay is a non-blocking hook error, Claude Code carries on), but a
; leftover gateway is not: with env.ANTHROPIC_BASE_URL still pointing at Zuko,
; Claude Code cannot reach Claude at all once Zuko is gone.
;
; So when Zuko's install state says it changed settings.json, the user is told
; how to undo it (Zuko → Settings → Protection, which restores the file exactly,
; including a previous ANTHROPIC_BASE_URL) and may stop the uninstall to do so.
; The install state itself is kept, so a later Zuko can still restore exactly.

!macro NSIS_HOOK_PREUNINSTALL
  IfFileExists "$LOCALAPPDATA\Zuko\install-state.json" 0 zuko_settings_clean
    MessageBox MB_YESNO|MB_ICONEXCLAMATION|MB_DEFBUTTON2 \
      "Zuko has changed Claude Code's settings (~/.claude/settings.json): hooks, the Zuko gateway or deny rules.$\r$\n$\r$\nIf the gateway is on, Claude Code will not be able to reach Claude after Zuko is removed.$\r$\n$\r$\nTo undo the changes exactly, choose No, open Zuko, go to Settings > Protection and turn everything off, then uninstall again.$\r$\n$\r$\nUninstall anyway?" \
      /SD IDYES IDYES zuko_settings_clean
    Abort
  zuko_settings_clean:
  RMDir /r "$LOCALAPPDATA\Zuko\bin"
  RMDir /r "$LOCALAPPDATA\Zuko\inbox"
  Delete "$LOCALAPPDATA\Zuko\zuko.log"
  ; The browser bridge the app registered for this user (nativehost.rs): only Zuko's own
  ; host name under each browser's NativeMessagingHosts key, and its manifest.
  DeleteRegKey HKCU "Software\Google\Chrome\NativeMessagingHosts\app.zuko.host"
  DeleteRegKey HKCU "Software\Microsoft\Edge\NativeMessagingHosts\app.zuko.host"
  DeleteRegKey HKCU "Software\Chromium\NativeMessagingHosts\app.zuko.host"
  DeleteRegKey HKCU "Software\BraveSoftware\Brave-Browser\NativeMessagingHosts\app.zuko.host"
  RMDir /r "$LOCALAPPDATA\Zuko\native-host"
!macroend
