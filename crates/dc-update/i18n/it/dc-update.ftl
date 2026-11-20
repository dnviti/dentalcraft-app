# Aggiornamenti di DentalCraft (italiano).

## Comandi (dentalcraft-cli, MCP, bridge dell'app)
app-cmd-update-check = Cerca aggiornamenti
app-cmd-update-check-doc = Chiede al sito di DentalCraft la versione più recente e dice come si aggiorna questa installazione. Risultato: current, latest, update_available, kind (standalone, appimage, deb, rpm, flatpak, exe, msi, macos-app o unknown), action (install, package-manager, manual o none) e notes_url; con package-manager anche command, il comando da eseguire; con install anche download (url, size); con manual anche download_page. Senza connessione non riesce e non cambia nulla.
app-cmd-update-apply = Aggiorna DentalCraft
app-cmd-update-apply-doc = Installa la versione più recente quando DentalCraft può farlo da solo (cartella standalone, AppImage, installazione Windows o MSI, app macOS): la scarica, controlla che sia autentica e completa e la mette al suo posto; se un controllo non va a buon fine la versione installata resta com'è. Le installazioni con apt, dnf e Flatpak le aggiorna il loro gestore di pacchetti: non si scarica nulla e il risultato indica il comando. In dentalcraft-cli la nuova versione si installa subito (un programma di installazione di Windows parte alla fine del comando). Nell'app si installa alla chiusura dell'app, oppure subito con restart impostato a true: il progetto viene salvato, l'app si chiude e si apre la nuova versione. Risultato: quello di app.update.check più updated.

## Errori
update-error-offline = DentalCraft non è riuscito a raggiungere il suo sito. Controlla la connessione a internet e riprova.
update-error-server = Il sito di DentalCraft non ha risposto come previsto. Riprova più tardi.
update-error-not-genuine = Non è stato possibile confermare che l'aggiornamento provenga da DentalCraft, quindi è stato ignorato. Non è stato installato nulla.
update-error-damaged = Il file scaricato è danneggiato o incompleto: è stato eliminato e non è stato installato nulla. Riprova.
update-error-cancelled = Il download è stato interrotto. Non è stato installato nulla.
update-error-not-writable = DentalCraft non può aggiungere file in { $path }. Scarica la nuova versione dal sito, oppure sposta DentalCraft in una cartella che puoi modificare.
update-error-no-download = Per questa copia di DentalCraft non c'è un download della nuova versione. Scaricala dal sito.
update-error-install = Non è stato possibile installare la nuova versione. La versione che hai è rimasta invariata.
