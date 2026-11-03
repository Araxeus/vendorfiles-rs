# Installs vendor on Windows, with a global config in ~\vendor.yml.
#
#   irm https://raw.githubusercontent.com/Araxeus/vendorfiles-rs/main/install.ps1 | iex
#
# Runs under Windows PowerShell 5.1 and PowerShell 7. Errors throw instead of calling `exit`,
# because `iex` runs in the user's session and `exit` would close their window.

function Install-Vendor {
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'

    # Windows PowerShell 5.1 may not offer TLS 1.2 by default, and GitHub requires it.
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

    $repository = 'https://github.com/Araxeus/vendorfiles-rs'

    # A 32-bit PowerShell reports its own architecture here and the real one in the W6432 variable.
    $architecture = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
    if ($architecture -ne 'AMD64') {
        throw "no release for Windows on $architecture. See $repository#installation for other ways to install."
    }

    $config = Join-Path $env:USERPROFILE 'vendor.yml'

    if (Test-Path -LiteralPath $config) {
        $folder = Get-ConfiguredFolder $config
        if (-not $folder) {
            throw "could not read vendorConfig.vendorFolder from $config. Set it to a plain or quoted path, then run this again."
        }
    } else {
        $folder = Join-Path $env:USERPROFILE '.local\bin'
    }

    # A config can spell the folder with forward slashes, which would not match a Path entry.
    $folder = [IO.Path]::GetFullPath($folder)

    New-Item -ItemType Directory -Force -Path $folder | Out-Null
    $binary = Join-Path $folder 'vendor.exe'

    # A binary already in place is left to `vendor add`, which knows its version from the lockfile.
    if (-not (Test-Path -LiteralPath $binary)) {
        Install-Binary $repository $folder
    }

    if (-not (Test-Path -LiteralPath $config)) {
        Write-Config $config $folder
    }

    & $binary --config $config --plain add vendorfiles
    if ($LASTEXITCODE -ne 0) {
        throw "vendor add vendorfiles failed with exit code $LASTEXITCODE"
    }

    Write-Host ''
    Write-Host "vendor is in $folder"
    Write-Host "Its config is $config"
    Write-Host ''
    Update-Environment $folder $config
    if (-not (Test-InstallsIntoFolder $config $folder)) {
        Write-DefaultsWarning $config $folder
    }
    Write-Host ''
    Write-Host 'Then install programs with: vendor add <name>'
}

# Returns the raw text after `vendorFolder:` in a top-level section of the config.
function Get-ConfigValue($config, $section) {
    $inside = $false
    foreach ($line in [IO.File]::ReadAllLines($config)) {
        if ($line.StartsWith("${section}:")) {
            $inside = $true
        } elseif ($line -match '^[^\s#]') {
            $inside = $false
        } elseif ($inside -and $line -cmatch '^\s+vendorFolder:\s*(.*)$') {
            return $Matches[1]
        }
    }
    return ''
}

# Decodes a YAML scalar. Returns $null for a null value or a form this script does not read,
# because guessing could send the install somewhere other than where vendor will look.
function ConvertFrom-Scalar($raw) {
    if ($raw -match "^'((?:[^']|'')*)'\s*(#.*)?$") {
        return $Matches[1] -replace "''", "'"
    }
    if ($raw -match '^"((?:[^"\\]|\\.)*)"\s*(#.*)?$') {
        $value = $Matches[1]
        if (($value -replace '\\[\\"]', '') -match '\\') { return $null }
        return [regex]::Replace($value, '\\(.)', '$1')
    }
    $value = ($raw -replace '\s+#.*$', '').Trim()
    if ($value -match '^(~|null|false|0)?$' -or $value -match '^[#\[{&*!|>%@`''"]') { return $null }
    return $value
}

# Resolves ~ and relative paths in a vendorFolder value the way vendor does.
function Resolve-Folder($config, $value) {
    $userHome = if ($env:HOME) { $env:HOME } else { $env:USERPROFILE }
    if ($value -eq '~') { return $userHome }
    if ($value -match '^~[/\\](.*)$') { return Join-Path $userHome $Matches[1] }
    if ([IO.Path]::IsPathRooted($value)) { return $value }
    return Join-Path (Split-Path -Parent $config) ($value -replace '^\.[/\\]', '')
}

function Get-ConfiguredFolder($config) {
    $value = ConvertFrom-Scalar (Get-ConfigValue $config 'vendorConfig')
    if (-not $value) { return $null }
    return Resolve-Folder $config $value
}

# vendor reads defaultVendorOptions when the config has one, and default otherwise.
function Get-DefaultSection($config) {
    foreach ($line in [IO.File]::ReadAllLines($config)) {
        if ($line.StartsWith('defaultVendorOptions:')) { return 'defaultVendorOptions' }
    }
    return 'default'
}

# Returns whether `vendor add` puts programs straight into the folder, not a subfolder of it.
function Test-InstallsIntoFolder($config, $folder) {
    $value = ConvertFrom-Scalar (Get-ConfigValue $config (Get-DefaultSection $config))
    if ($value -eq '{vendorFolder}') { return $true }
    if (-not $value) { return $false }
    return [IO.Path]::GetFullPath((Resolve-Folder $config $value)).TrimEnd('\') -ieq $folder.TrimEnd('\')
}

function Write-DefaultsWarning($config, $folder) {
    Write-Host ''
    Write-Host "warning: $config sets no default vendorFolder, so vendor add will put each program in"
    Write-Host "its own subfolder of $folder, which is not on your Path. To keep programs on Path, add:"
    Write-Host "  $(Get-DefaultSection $config):"
    Write-Host "    vendorFolder: '{vendorFolder}'"
}

function Install-Binary($repository, $folder) {
    # The redirect of /releases/latest names the tag and uses no GitHub API quota.
    $request = [Net.WebRequest]::Create("$repository/releases/latest")
    $request.Method = 'HEAD'
    $response = $request.GetResponse()
    $tag = $response.ResponseUri.Segments[-1]
    $response.Close()
    if ($tag -notmatch '^v') {
        throw "could not read the latest version from $repository"
    }

    $work = Join-Path ([IO.Path]::GetTempPath()) ([IO.Path]::GetRandomFileName())
    New-Item -ItemType Directory -Path $work | Out-Null
    try {
        $archive = Join-Path $work 'vendor.zip'
        $version = $tag.Substring(1)
        Invoke-WebRequest -UseBasicParsing -Uri "$repository/releases/download/$tag/vendor_v${version}_windows.zip" -OutFile $archive
        Expand-Archive -LiteralPath $archive -DestinationPath $work
        Move-Item -LiteralPath (Join-Path $work 'vendor.exe') -Destination $folder
    } finally {
        Remove-Item -LiteralPath $work -Recurse -Force
    }
}

function Write-Config($config, $folder) {
    $path = ($folder -replace '\\', '/') -replace "'", "''"
    $text = "vendorConfig:`n  vendorFolder: '$path'`ndefault:`n  vendorFolder: '{vendorFolder}'`n  locked: false`n"

    # Set-Content on Windows PowerShell 5.1 would add a byte order mark.
    [IO.File]::WriteAllText($config, $text)
}

function Test-PathEntry($path, $folder) {
    $wanted = $folder.TrimEnd('\')
    foreach ($entry in ($path -split ';')) {
        if ($entry.TrimEnd('\') -ieq $wanted) { return $true }
    }
    return $false
}

# Sets the user's environment and the current session's, and reports what changed.
function Update-Environment($folder, $config) {
    $changed = @()

    # The Path value is read and written raw, so entries like %USERPROFILE%\bin stay unexpanded.
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
    try {
        $userPath = [string]$key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        $machinePath = [Environment]::GetEnvironmentVariable('Path', 'Machine')
        if (-not (Test-PathEntry $userPath $folder) -and -not (Test-PathEntry $machinePath $folder)) {
            $newPath = if ($userPath) { "$($userPath.TrimEnd(';'));$folder" } else { $folder }
            $key.SetValue('Path', $newPath, [Microsoft.Win32.RegistryValueKind]::ExpandString)
            $changed += "Path: added $folder"
        }
    } finally {
        $key.Close()
    }

    $previous = [Environment]::GetEnvironmentVariable('DEFAULT_VENDOR_CONFIG', 'User')
    if ($previous -ne $config) {
        $changed += if ($previous) { "DEFAULT_VENDOR_CONFIG: $config (was $previous)" } else { "DEFAULT_VENDOR_CONFIG: $config" }
    }

    # This call also tells open programs that the environment changed, which covers Path too.
    [Environment]::SetEnvironmentVariable('DEFAULT_VENDOR_CONFIG', $config, 'User')

    $env:DEFAULT_VENDOR_CONFIG = $config
    if (-not (Test-PathEntry $env:Path $folder)) {
        $env:Path = "$folder;$env:Path"
    }

    if ($changed.Count -eq 0) {
        Write-Host 'Your user environment already has the settings vendor needs:'
        Write-Host "  Path: includes $folder"
        Write-Host "  DEFAULT_VENDOR_CONFIG: $config"
        return
    }

    Write-Host 'Changed in your user environment:'
    foreach ($line in $changed) {
        Write-Host "  $line"
    }
    Write-Host ''
    Write-Host 'This window can use vendor now. Windows that were already open need a restart.'
}

Install-Vendor
