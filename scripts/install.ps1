param([string]$Version = "latest", [string]$Destination = "$env:LOCALAPPDATA\openproject\bin")
$ErrorActionPreference = "Stop"
$Repository = if ($env:OPENPROJECT_RELEASE_REPOSITORY) { $env:OPENPROJECT_RELEASE_REPOSITORY } else { "yungts97/openproject-skill" }
$Target = if ([System.Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture -eq "Arm64") { "aarch64-pc-windows-msvc" } else { "x86_64-pc-windows-msvc" }
$Archive = "openproject-$Target.zip"; $Temporary = Join-Path ([System.IO.Path]::GetTempPath()) ("openproject-" + [guid]::NewGuid())
New-Item -ItemType Directory -Force -Path $Temporary | Out-Null
try {
  if ($env:OPENPROJECT_GITLAB_PROJECT) { if ($Version -eq "latest") { throw "Supply an explicit release tag when using OPENPROJECT_GITLAB_PROJECT." }; if (-not (Get-Command glab -ErrorAction SilentlyContinue)) { throw "glab is required for OPENPROJECT_GITLAB_PROJECT." }; if ($env:OPENPROJECT_GITLAB_HOST) { & glab release download $Version --hostname $env:OPENPROJECT_GITLAB_HOST --repo $env:OPENPROJECT_GITLAB_PROJECT --pattern $Archive --pattern "SHA256SUMS" --dir $Temporary } else { & glab release download $Version --repo $env:OPENPROJECT_GITLAB_PROJECT --pattern $Archive --pattern "SHA256SUMS" --dir $Temporary } }
  else { $base = if ($Version -eq "latest") { "https://github.com/$Repository/releases/latest/download" } else { "https://github.com/$Repository/releases/download/v$Version" }; Invoke-WebRequest "$base/$Archive" -OutFile (Join-Path $Temporary $Archive); Invoke-WebRequest "$base/SHA256SUMS" -OutFile (Join-Path $Temporary "SHA256SUMS") }
  $line = Get-Content (Join-Path $Temporary "SHA256SUMS") | Where-Object { $_ -match [regex]::Escape($Archive) } | Select-Object -First 1; $expected = $line.Split()[0]
  if (-not $expected) { throw "No checksum found for $Archive." }; $actual = (Get-FileHash (Join-Path $Temporary $Archive) -Algorithm SHA256).Hash.ToLower()
  if ($actual -ne $expected.ToLower()) { throw "Checksum verification failed." }; New-Item -ItemType Directory -Force -Path $Destination | Out-Null; Expand-Archive (Join-Path $Temporary $Archive) -DestinationPath $Destination -Force; Write-Output "Installed $(Join-Path $Destination 'openproject.exe')"
} finally { Remove-Item -Recurse -Force $Temporary -ErrorAction SilentlyContinue }
