# Diagnostica anonima di DentalCraft (italiano).

## Comandi (dentalcraft-cli, MCP, ponte dell'app)
app-cmd-diagnostics-status = Stato della diagnostica
app-cmd-diagnostics-status-doc = Dice se DentalCraft invia dati diagnostici anonimi. Risultato: enabled (true, false, oppure null se l'utente non ha ancora scelto), install_id (il numero casuale di questa installazione finché la diagnostica è attiva), pending (rapporti in attesa di invio) ed endpoint (dove vengono inviati). Non cambia nulla.
app-cmd-diagnostics-set = Disattiva la diagnostica
app-cmd-diagnostics-set-doc = Disattiva l'invio di dati diagnostici anonimi, come l'interruttore nella finestra Impostazioni, e cancella i rapporti non ancora inviati e il numero casuale di installazione. Parametro: enabled, che deve essere false. L'attivazione viene rifiutata con l'errore consent-only-in-app: i dati vengono inviati solo dopo che l'utente ha spuntato la casella nell'app (la domanda al primo avvio o Impostazioni). Risultato: quello di app.diagnostics.status.
app-cmd-diagnostics-preview = Mostra cosa viene inviato
app-cmd-diagnostics-preview-doc = Mostra il prossimo rapporto esattamente come verrebbe inviato ora, con i dati personali già tolti: report (null se non c'è nulla in attesa), pending ed enabled. Non invia nulla.
app-cmd-diagnostics-send = Invia ora la diagnostica
app-cmd-diagnostics-send-doc = Invia subito i rapporti in attesa invece che più tardi. Risultato: sent, accepted, duplicates, dropped (rifiutati dal server) e remaining. Non riesce se la diagnostica è disattivata o il server non è raggiungibile; i rapporti allora restano e vengono inviati più tardi.

## settings
app-cmd-settings-get = Impostazioni
app-cmd-settings-get-doc = Restituisce le impostazioni della finestra Impostazioni: advanced (gli ispettori mostrano tutte le opzioni e i parametri), language (en o it), theme (dark, light o system), check_updates (cerca una versione più recente all'avvio) e diagnostics (true, false, o null se l'utente non ha ancora scelto). Non cambia nulla.
app-cmd-settings-set = Cambia le impostazioni
app-cmd-settings-set-doc = Cambia un'impostazione della finestra Impostazioni, come fa il suo interruttore; viene salvata per l'avvio successivo. Parametro: advanced (true: gli ispettori mostrano tutte le opzioni e i parametri; false, il predefinito: solo le esecuzioni automatiche e le correzioni manuali più utili). I comandi mantengono comunque tutti i loro parametri. Risultato: quello di app.settings.get.
