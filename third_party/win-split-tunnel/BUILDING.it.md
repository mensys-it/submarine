[English](BUILDING.md) | Italiano

# Tunnel per app su Windows: compilare il driver da sé

Su Windows il tunnel per app richiede un driver kernel, quello di questa cartella.
L'installer di Submarine **non** lo include ancora: Windows carica solo i driver firmati da
Microsoft, e questo non lo è ancora. Tutto il resto (tunnel, kill switch, DNS, reti Wi-Fi
fidate) funziona anche senza.

Il driver si può comunque compilare, firmare con un proprio certificato di test e usare con
Windows in **modalità test**.

> **Prima di iniziare.** La modalità test abbassa la protezione del computer: Windows carica
> qualsiasi driver firmato con un certificato di cui il computer si fida, e il Secure Boot
> del firmware va disattivato. Sul desktop compare la scritta "Modalità test", gli anti-cheat
> di alcuni giochi e alcune policy aziendali non funzionano su un computer così configurato,
> e BitLocker può chiedere la chiave di ripristino al riavvio successivo. Usala solo su un
> computer tuo, se accetti tutto questo.

## 1. Compilare il driver

Servono `submarine-split-tunnel.sys` firmato e il certificato `submarine-test-driver.cer` con
cui è stato firmato. Ci sono due modi per ottenerli.

### Con GitHub Actions (senza installare strumenti)

1. Fai un fork di questo repository su GitHub.
2. Nel fork, apri la scheda **Actions** e abilita i workflow (nei fork sono spenti).
3. Scegli **windows-driver** → **Run workflow**.
4. Al termine, scarica dalla pagina dell'esecuzione l'artifact
   `submarine-split-tunnel-driver` ed estrailo.

Ogni esecuzione crea un nuovo certificato di test, la cui chiave privata sparisce con il
runner.

### Su un computer Windows

Installa Visual Studio 2022 con il carico di lavoro **Sviluppo di applicazioni desktop con
C++**, il Windows SDK e il Windows Driver Kit (WDK). Poi, dalla cartella del repository:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\windows\build-driver.ps1
```

I due file vengono scritti in `dist\driver`. Lo script lascia il certificato di test, con la
sua chiave privata, nell'archivio certificati personale: chi ha quella chiave può firmare
driver di cui il tuo computer si fiderà, quindi eliminalo a build finita:

```powershell
Get-ChildItem Cert:\CurrentUser\My | Where-Object Subject -eq "CN=Submarine Test Driver" | Remove-Item
```

## 2. Attivare la modalità test

1. Disattiva il **Secure Boot** nelle impostazioni del firmware (UEFI/BIOS) del computer. Se
   il disco è cifrato con BitLocker, prima assicurati di avere la chiave di ripristino,
   oppure sospendi BitLocker per i prossimi riavvii:

   ```powershell
   Suspend-BitLocker -MountPoint "C:" -RebootCount 2
   ```

2. Da PowerShell **come amministratore**, nella cartella con i due file:

   ```powershell
   bcdedit /set testsigning on
   Import-Certificate -FilePath .\submarine-test-driver.cer -CertStoreLocation Cert:\LocalMachine\Root
   Import-Certificate -FilePath .\submarine-test-driver.cer -CertStoreLocation Cert:\LocalMachine\TrustedPublisher
   ```

## 3. Installare il driver

Installa Submarine con il suo installer come al solito, poi copia il driver accanto al
servizio, sempre da PowerShell come amministratore:

```powershell
Copy-Item .\submarine-split-tunnel.sys "$env:ProgramFiles\Submarine\"
Restart-Computer
```

Dopo il riavvio compare "Modalità test" in un angolo del desktop. Scegli le app nella pagina
**Protezione** di Submarine: il servizio carica il driver la prima volta che si attiva il
tunnel per app. Per controllarlo:

```powershell
sc.exe query SubmarineSplitTunnel
```

Se il driver non parte, Submarine mostra l'errore nell'app: controlla che la modalità test
sia attiva (`bcdedit` elenca `testsigning Yes`) e che il certificato sia quello della stessa
build.

## Aggiornamenti e rimozione

- **Gli aggiornamenti di Submarine** lasciano il driver al suo posto, perché l'installer non
  lo conosce.
- **Una nuova build del driver**: ferma il servizio (`Stop-Service Submarine`), sostituisci il
  file, rendi attendibile il nuovo certificato come al passo 2 e riavvia il computer.
- **Rimozione**: la disinstallazione di Submarine rimuove il servizio del driver ma lascia
  `submarine-split-tunnel.sys` in `Program Files\Submarine`: elimina quella cartella. Poi
  disattiva la modalità test, rimuovi il certificato e riattiva il Secure Boot nel firmware:

  ```powershell
  bcdedit /set testsigning off
  Get-ChildItem Cert:\LocalMachine\Root, Cert:\LocalMachine\TrustedPublisher |
      Where-Object Subject -eq "CN=Submarine Test Driver" | Remove-Item
  ```

## Per i manutentori

- Per firmare ogni build della CI con lo stesso certificato di test, così che i tester lo
  rendano attendibile una volta sola, salvalo nei secret del repository
  `SUBMARINE_DRIVER_PFX` (il file `.pfx` in base64) e `SUBMARINE_DRIVER_PFX_PASSWORD`.
  `build-driver.ps1` legge le stesse variabili, oppure accetta `-PfxPath` e `-PfxPassword`.
- `scripts/build-windows.sh` include il driver nell'installer quando
  `dist/driver/submarine-split-tunnel.sys` esiste (o il file indicato da
  `SUBMARINE_DRIVER_SYS`).
- Distribuire il driver a tutti richiede l'attestation signing di Microsoft: un certificato EV
  di firma del codice, un account nel programma Windows Hardware Developer di Partner Center e
  l'invio del driver in un file `.cab`.
