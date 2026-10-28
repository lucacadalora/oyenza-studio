Oyenza Studio test copy
=======================

This folder is the Microsoft Store package of Oyenza Studio, signed with a
throwaway certificate by the CI run that built it. Install it to try what
only the Store copy can do, such as the Notifications add-on: Windows lets
only an app installed as a package read other apps' notifications.

It's a test build, not the Store's: never upload it anywhere.

Install
-------
1. Unzip this folder.
2. Right-click Install-Test-Copy.ps1 > Run with PowerShell.
   (Or in PowerShell, in this folder:
    powershell -ExecutionPolicy Bypass -File .\Install-Test-Copy.ps1)
3. Windows asks for administrator rights once: that trusts this one
   certificate for installing apps. Its private key was thrown away with the
   CI run, so nothing else can be signed with it.
4. The script quits the Oyenza Studio that's running (its settings stay),
   installs the test copy and starts it. The test copy keeps its own
   settings, so it starts like a new install.

Try Notifications
-----------------
1. Open the island, press + (Add-ons) and switch Notifications on.
2. Windows asks whether Oyenza Studio may read your notifications: Allow.
   (No question? The bell tab's page has Ask again and Windows settings.)
3. WhatsApp, Telegram and Phone Link start on. In Telegram Desktop, turn on
   Settings > Notifications and Sounds > Use Windows notifications.
4. A message shows on the island for 8 seconds: Open (or a click on it)
   opens the app. The bell tab lists every app that sent one, with a switch,
   and two more: Who it's from and Message text (off: they wait for Show).

Go back
-------
Run Remove-Test-Copy.ps1 the same way. It removes the test copy and its
settings, stops trusting the certificate (administrator rights again) and
starts your usual Oyenza Studio.

While the test copy is installed it also starts with Windows. Only one
island runs at a time: whichever starts first.
