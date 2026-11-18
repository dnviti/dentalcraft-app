# Diagnostica anonima di DentalCraft (italiano).

## Comandi (dentalcraft-cli, MCP, ponte dell'app)
app-cmd-diagnostics-status = Stato della diagnostica
app-cmd-diagnostics-status-doc = Dice se DentalCraft invia dati diagnostici anonimi. Risultato: enabled (true, false, oppure null se l'utente non ha ancora scelto), install_id (il numero casuale di questa installazione finché la diagnostica è attiva), pending (rapporti in attesa di invio) ed endpoint (dove vengono inviati). Non cambia nulla.
app-cmd-diagnostics-set = Attiva o disattiva la diagnostica
app-cmd-diagnostics-set-doc = Attiva o disattiva l'invio di dati diagnostici anonimi, come la casella del menu account. Attivandola si crea un nuovo numero casuale di installazione; disattivandola si cancellano i rapporti non ancora inviati e quel numero. Parametro: enabled (true o false). Risultato: quello di app.diagnostics.status.
app-cmd-diagnostics-preview = Mostra cosa viene inviato
app-cmd-diagnostics-preview-doc = Mostra il prossimo rapporto esattamente come verrebbe inviato ora, con i dati personali già tolti: report (null se non c'è nulla in attesa), pending ed enabled. Non invia nulla.
app-cmd-diagnostics-send = Invia ora la diagnostica
app-cmd-diagnostics-send-doc = Invia subito i rapporti in attesa invece che più tardi. Risultato: sent, accepted, duplicates, dropped (rifiutati dal server) e remaining. Non riesce se la diagnostica è disattivata o il server non è raggiungibile; i rapporti allora restano e vengono inviati più tardi.
