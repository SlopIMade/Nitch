# Creates the signing key for the TV app once and stores it as GitHub Actions secrets, so the
# release workflow (.github/workflows/release.yml) signs every APK with the same key and Android
# installs updates over the previous version.
#
#   .\scripts\create-apk-key.ps1 -Out D:\safe\place
#
# Writes nitch-release.jks and nitch-release-passwords.txt to -Out: back both up somewhere safe
# and never commit them. A lost key means users have to uninstall the app before the next update.
# Needs: the JDK's keytool (Android Studio's is found automatically) and gh, logged in with access
# to the repository.
param(
    [Parameter(Mandatory = $true)][string]$Out,
    [string]$Repo = "SlopIMade/Nitch",
    [string]$Alias = "nitch"
)

$keytool = (Get-Command keytool -ErrorAction SilentlyContinue).Source
if (-not $keytool) { $keytool = "C:\Program Files\Android\Android Studio\jbr\bin\keytool.exe" }
if (-not (Test-Path $keytool)) { throw "keytool not found: install a JDK or Android Studio" }
if (-not (Get-Command gh -ErrorAction SilentlyContinue)) { throw "gh (GitHub CLI) not found" }

New-Item -ItemType Directory -Force $Out | Out-Null
$ks = Join-Path $Out "nitch-release.jks"
if (Test-Path $ks) { throw "$ks already exists: keep using it (or move it away to create a new key)" }

# One random password for the keystore and the key (PKCS12 uses the same for both).
$bytes = New-Object byte[] 24
[System.Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($bytes)
$password = [Convert]::ToBase64String($bytes) -replace '[+/=]', 'x'

# keytool reports progress on stderr; that is not an error.
& $keytool -genkeypair -keystore $ks -storetype PKCS12 -alias $Alias -keyalg RSA -keysize 4096 `
    -validity 10000 -storepass $password -keypass $password -dname "CN=Nitch" 2>$null
if ($LASTEXITCODE -ne 0 -or -not (Test-Path $ks)) { throw "keytool failed" }

$notes = Join-Path $Out "nitch-release-passwords.txt"
Set-Content -Encoding utf8 -Path $notes -Value @(
    "Nitch TV app signing key ($ks)",
    "alias: $Alias",
    "keystore and key password: $password"
)

# The secrets the release workflow reads.
[Convert]::ToBase64String([IO.File]::ReadAllBytes($ks)) | gh secret set NITCH_KEYSTORE_BASE64 --repo $Repo
$password | gh secret set NITCH_KEYSTORE_PASSWORD --repo $Repo
$password | gh secret set NITCH_KEY_PASSWORD --repo $Repo
if ($LASTEXITCODE -ne 0) { throw "gh secret set failed" }

Write-Host ""
Write-Host "Done. Key: $ks"
Write-Host "Passwords: $notes"
Write-Host "Secrets set on $Repo. Back up both files; never commit them."
