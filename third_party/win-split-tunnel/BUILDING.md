English | [Italiano](BUILDING.it.md)

# Per-app tunnel on Windows: build the driver yourself

On Windows the per-app tunnel needs a kernel driver, the one in this folder. The Submarine
installer does **not** include it yet: Windows loads only drivers signed by Microsoft, and
this one is not signed by Microsoft yet. Everything else (tunnels, kill switch, DNS, trusted
Wi-Fi networks) works without it.

You can still build the driver, sign it with a test certificate of your own and run it with
Windows in **test mode**.

> **Before you start.** Test mode lowers the protection of the computer: Windows loads any
> driver signed with a certificate the computer trusts, and the firmware's Secure Boot must
> be turned off. "Test Mode" is shown on the desktop, some games' anti-cheat systems and some
> company policies refuse to run on such a computer, and BitLocker may ask for its recovery
> key at the next start. Use it only on a computer of your own where you accept this.

## 1. Build the driver

You need the signed `submarine-split-tunnel.sys` and the `submarine-test-driver.cer`
certificate it was signed with. There are two ways to get them.

### With GitHub Actions (no tools to install)

1. Fork this repository on GitHub.
2. In the fork, open the **Actions** tab and enable workflows (forks start with them off).
3. Choose **windows-driver** → **Run workflow**.
4. When the run ends, download the `submarine-split-tunnel-driver` artifact from its page
   and extract it.

Every run creates a new test certificate, whose private key is discarded with the runner.

### On a Windows computer

Install Visual Studio 2022 with the **Desktop development with C++** workload, the Windows
SDK and the Windows Driver Kit (WDK). Then, from the repository folder:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\windows\build-driver.ps1
```

The two files are written to `dist\driver`. The script leaves the test certificate, with its
private key, in your personal certificate store: whoever has that key can sign drivers your
computer will trust, so delete it once the build is done:

```powershell
Get-ChildItem Cert:\CurrentUser\My | Where-Object Subject -eq "CN=Submarine Test Driver" | Remove-Item
```

## 2. Turn on test mode

1. Turn off **Secure Boot** in the firmware settings (UEFI/BIOS) of the computer. If the
   disk is encrypted with BitLocker, first make sure you have the recovery key, or suspend
   BitLocker for the next restarts:

   ```powershell
   Suspend-BitLocker -MountPoint "C:" -RebootCount 2
   ```

2. From PowerShell **as administrator**, in the folder with the two files:

   ```powershell
   bcdedit /set testsigning on
   Import-Certificate -FilePath .\submarine-test-driver.cer -CertStoreLocation Cert:\LocalMachine\Root
   Import-Certificate -FilePath .\submarine-test-driver.cer -CertStoreLocation Cert:\LocalMachine\TrustedPublisher
   ```

## 3. Install the driver

Install Submarine with its installer as usual, then copy the driver next to the service,
again from PowerShell as administrator:

```powershell
Copy-Item .\submarine-split-tunnel.sys "$env:ProgramFiles\Submarine\"
Restart-Computer
```

After the restart "Test Mode" appears in a corner of the desktop. Choose the apps in the
**Protection** page of Submarine: the service loads the driver the first time the per-app
tunnel is turned on. To check it:

```powershell
sc.exe query SubmarineSplitTunnel
```

If the driver does not start, Submarine shows the error in the app: check that test mode is
on (`bcdedit` lists `testsigning Yes`) and that the certificate is the one of the same build.

## Updates and removal

- **Submarine updates** leave the driver in place, since the installer does not know about
  it.
- **A new build of the driver**: stop the service (`Stop-Service Submarine`), replace the
  file, trust the new certificate as in step 2 and restart the computer.
- **Removal**: uninstalling Submarine removes the driver service but leaves
  `submarine-split-tunnel.sys` in `Program Files\Submarine`: delete that folder. Then turn
  test mode off, remove the certificate and turn Secure Boot back on in the firmware:

  ```powershell
  bcdedit /set testsigning off
  Get-ChildItem Cert:\LocalMachine\Root, Cert:\LocalMachine\TrustedPublisher |
      Where-Object Subject -eq "CN=Submarine Test Driver" | Remove-Item
  ```

## For maintainers

- To sign every CI build with the same test certificate, so testers trust it only once,
  store it as the repository secrets `SUBMARINE_DRIVER_PFX` (the `.pfx` file in base64) and
  `SUBMARINE_DRIVER_PFX_PASSWORD`. `build-driver.ps1` reads the same variables, or takes
  `-PfxPath` and `-PfxPassword`.
- `scripts/build-windows.sh` includes the driver in the installer when
  `dist/driver/submarine-split-tunnel.sys` exists (or the file named by
  `SUBMARINE_DRIVER_SYS`).
- Shipping the driver to everyone needs Microsoft's attestation signing: an EV code signing
  certificate, an account in the Windows Hardware Developer program of Partner Center, and the
  driver sent there in a `.cab` file.
