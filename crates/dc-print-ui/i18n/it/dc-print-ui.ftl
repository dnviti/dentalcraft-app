# Finestra di stampa: scegliere gli allineatori, disporli sul piatto della stampante, salvare i file di stampa.
print-title = Stampa allineatori
print-close = Chiudi la finestra di stampa
print-prepare = Prepara la stampa
print-hint-no-resin = Inserisci prima i valori della tua resina: la stampa ne ha bisogno.
print-hint-form-open = Prima salva o annulla i valori della resina.
print-hint-no-parts = Scegli almeno un allineatore da stampare.

print-printer = Stampante
print-printer-profile = Profilo stampante { $profile }: la vasca della resina si scalda a { $c } °C.
print-printer-usb = I file arrivano alla stampante su una chiavetta USB: non serve la rete.

print-resin = Resina
print-resin-summary = Strati da { $layer } mm, { $exposure } s ciascuno; primi { $bottom } strati { $bottom_exposure } s.
print-resin-change = Cambia i valori della resina
print-resin-none = Nessuna resina
print-resin-none-body = Inserisci i valori che il produttore della resina indica per questa stampante. Li inserisci una volta sola; restano per le stampe successive.
print-resin-enter = Inserisci i valori della resina

print-form-intro = Copia questi valori dalla scheda del produttore della resina per questa stampante.
print-form-name = Nome della resina
print-form-name-hint = es. Resina trasparente per allineatori
print-form-layer = Altezza dello strato (mm)
print-form-exposure = Esposizione di ogni strato (s)
print-form-bottom-exposure = Esposizione dei primi strati (s)
print-form-bottom-exposure-help = I primi strati restano accesi più a lungo, così la stampa resta attaccata al piatto.
print-form-bottom-layers = Numero dei primi strati
print-form-rest = Pausa prima di ogni esposizione (s)
print-form-rest-help = Lascia assestare la resina prima di accendere ogni strato.
print-form-density = Densità della resina (g/ml)
print-form-save = Salva i valori della resina
print-form-cancel = Annulla
print-form-err-name = Usa da 1 a 31 lettere semplici, cifre o spazi (senza accenti).
print-form-err-layer = Usa un'altezza tra { $min } e { $max } mm.
print-form-err-positive = Inserisci un numero maggiore di 0.
print-form-err-zero-or-more = Inserisci un numero uguale o maggiore di 0.
print-form-err-count = Inserisci un numero intero.

print-adv-settings = Impostazioni di stampa
print-adv-supports = Supporti
print-adv-tip = Punta dove il supporto tocca
print-adv-shaft = Spessore dei supporti
print-adv-raft = Base sotto i supporti
print-adv-plate = Spazi sul piatto
print-adv-gap = Spazio tra gli allineatori
print-adv-margin = Spazio dal bordo del piatto

print-parts = Allineatori
print-parts-none = Non c'è ancora niente da stampare.
print-parts-chosen = { $n } di { $of } scelti
print-parts-missing = Scegli quelli da stampare
print-parts-all = Tutti
print-parts-none-chosen = Nessuno
print-part-no-file = Non ancora esportato: esegui prima "Esporta per la stampa".
print-status-missing = Da stampare
print-status-queued = In preparazione
print-status-sent = Inviato alla stampante
print-status-printed = Stampato
print-status-failed = Non riuscito
print-status-reprint = Da ristampare

print-plate = Piatto
print-plate-n = Piatto { $n } di { $of }
print-plate-next = Piatto successivo
print-plate-prev = Piatto precedente
print-plate-view = Il piatto visto dall'alto
print-plate-empty = Gli allineatori compaiono qui quando sono disposti sul piatto.

print-step-reading = Lettura degli allineatori…
print-step-placing = Orientamento degli allineatori, aggiunta dei supporti e disposizione sul piatto…
print-step-layers = Preparazione degli strati del piatto { $n } di { $of }…
print-step-saving = Salvataggio dei file…
print-cancel = Annulla

print-est-parts = Allineatori su questo piatto
print-est-layers = Strati
print-est-time = Tempo di stampa (circa)
print-est-resin = Resina
print-est-resin-needed = Resina da versare nella vasca (almeno)
print-ml = { $ml } ml
print-ml-g = { $ml } ml ({ $g } g)
print-total = Tutti i { $n } piatti
print-total-value = { $time }, { $ml } ml ({ $g } g)
print-time-m = { $m } min
print-time-hm = { $h } h { $m } min
print-preheat = La stampante prima scalda la vasca della resina a { $c } °C (circa { $time }), poi stampa: il tempo di stampa lo comprende.
print-preheat-no-target = La stampante prima scalda la vasca della resina (circa { $time }), poi stampa: il tempo di stampa lo comprende.

print-save = Salva i file di stampa…
print-save-help = Scegli una chiavetta USB (FAT32). Ogni piatto diventa un file.
print-save-models = Salva il piatto con i supporti come STL…
print-dialog-files = Scegli dove salvare i file di stampa
print-dialog-models = Scegli dove salvare i piatti come STL

print-failed = Non eseguito
print-cancelled = Interrotto
print-saved = Salvato
print-saved-files = { $n } file di stampa salvati in { $folder }.
print-saved-models = { $n } piatti salvati come STL in { $folder }.
print-next-steps = Inserisci la chiavetta USB nella stampante, scegli il file sul suo schermo e avvia la stampa.
print-after = Dopo la stampa: togli i supporti dal lato esterno, lava e polimerizza come indica il produttore della resina, poi controlla lo spessore delle pareti e l'adattamento prima dell'uso.

print-err-no-parts = Non c'è niente da stampare: scegli almeno un allineatore.
print-err-invalid-part = La forma di un allineatore non si può stampare così com'è. Esporta di nuovo gli allineatori.
print-err-invalid-settings = Un valore della resina è fuori dall'intervallo. Controlla i valori della resina.
print-err-fitting-contact = Un supporto toccherebbe l'interno di un allineatore. Prova altre impostazioni dei supporti.
print-err-island-unsupported = Una parte di un allineatore verrebbe stampata nel vuoto senza supporto. Prova altre impostazioni dei supporti.
print-err-part-too-tall = Un allineatore con i suoi supporti è più alto di quanto la stampante possa stampare.
print-err-part-too-big = Un allineatore non entra nel piatto. Riduci lo spazio dal bordo del piatto.
print-err-invalid-footprint = Non è stato possibile disporre un allineatore sul piatto. Esporta di nuovo gli allineatori.
print-err-invalid-layout = Gli spazi non lasciano posto sul piatto. Riduci gli spazi.
print-err-file = Non è stato possibile preparare il file di stampa.
print-err-io = Non è stato possibile scrivere un file.
print-err-mesh = Non è stato possibile salvare il piatto come STL.
print-err-read = Non è stato possibile leggere il file di un allineatore. Esporta di nuovo gli allineatori.
print-err-write = Non è stato possibile salvare i file lì. Controlla la chiavetta USB e che abbia spazio.
print-err-no-folder = Non c'è una cartella in cui salvare. Esporta prima gli allineatori.
print-err-internal = Qualcosa ha interrotto la preparazione. Riprova; se succede ancora, invia la diagnostica.
print-err-other = Non è stato possibile preparare la stampa.
