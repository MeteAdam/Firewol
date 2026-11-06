#Requires -RunAsAdministrator
param([Parameter(Mandatory)] [string] $ImagePath)

$ErrorActionPreference = 'Stop'
$path = [System.IO.Path]::GetFullPath($ImagePath)
if ([System.IO.Path]::GetExtension($path) -ne '.vhdx' -or (Test-Path -LiteralPath $path)) {
    throw 'Supply a new .vhdx path in an existing directory'
}

$vhd = New-VHD -Path $path -Dynamic -SizeBytes 4GB
try {
    $disk = Mount-VHD -Path $vhd.Path -Passthru | Get-Disk
    $attached = Get-VHD -DiskNumber $disk.Number
    if ($attached.Path -ne $path -or $disk.IsBoot -or $disk.IsSystem -or $disk.PartitionStyle -ne 'RAW') {
        throw 'The attached disk is not the new, empty Firewol virtual disk'
    }
    $partition = Initialize-Disk -InputObject $disk -PartitionStyle GPT -PassThru |
        New-Partition -UseMaximumSize -AssignDriveLetter
    $volume = Format-Volume -Partition $partition -FileSystem NTFS -NewFileSystemLabel 'Firewol M0' -Confirm:$false
    '{0}:\' -f $volume.DriveLetter
} catch {
    Dismount-VHD -Path $path -ErrorAction SilentlyContinue
    throw
}
