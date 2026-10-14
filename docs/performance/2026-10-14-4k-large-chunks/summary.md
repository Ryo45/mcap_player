# 4K and large Chunk measurements

Three fresh processes per condition; supported times/RSS are medians. Product limits remain 256 MiB.

| Case | Selection / mode | Status (3 runs) | Actual max Chunk MiB | Read MiB | Work ms / recording ms | JPEG decodes | Peak RSS MiB |
|---|---|---|---:|---:|---:|---:|---:|
| 3840x2160-8cam-10hz-none-8MiB-jpeg | scan / forward | ok, ok, ok | 9.05 | 356.65 | 269.82 / 4000 | 0.00 | 30.68 |
| 3840x2160-8cam-10hz-none-8MiB-jpeg | all / forward | ok, ok, ok | 9.05 | 950.17 | 7749.40 / 4000 | 179.00 | 450.70 |
| 3840x2160-8cam-10hz-none-64MiB-jpeg | scan / forward | ok, ok, ok | 65.53 | 950.15 | 675.83 / 4000 | 0.00 | 134.29 |
| 3840x2160-8cam-10hz-none-64MiB-jpeg | all / forward | ok, ok, ok | 65.53 | 950.15 | 7766.69 / 4000 | 179.00 | 501.39 |
| 3840x2160-8cam-10hz-none-128MiB-jpeg | scan / forward | product_limit, product_limit, product_limit | 130.80 | — | — / 4000 | — | 134.04 |
| 3840x2160-8cam-10hz-none-128MiB-jpeg | all / forward | product_limit, product_limit, product_limit | 130.80 | — | — / 4000 | — | 501.39 |
| 3840x2160-8cam-10hz-none-256MiB-jpeg | scan / forward | product_limit, product_limit, product_limit | 258.32 | — | — / 4000 | — | 3.47 |
| 3840x2160-8cam-10hz-none-256MiB-jpeg | all / forward | product_limit, product_limit, product_limit | 258.32 | — | — / 4000 | — | 3.48 |
| 3840x2160-8cam-10hz-none-512MiB-jpeg | scan / forward | product_limit, product_limit, product_limit | 513.84 | — | — / 4000 | — | 3.45 |
| 3840x2160-8cam-10hz-none-512MiB-jpeg | all / forward | product_limit, product_limit, product_limit | 513.84 | — | — / 4000 | — | 3.47 |
| 3840x2160-8cam-10hz-none-512MiB-jpeg | scan / indexed | product_limit, product_limit, product_limit | 513.84 | — | — / 4000 | — | 3.52 |
| 3840x2160-8cam-10hz-zstd-8MiB-jpeg | scan / forward | ok, ok, ok | 9.05 | 353.37 | 314.41 / 4000 | 0.00 | 40.11 |
| 3840x2160-8cam-10hz-zstd-8MiB-jpeg | all / forward | ok, ok, ok | 9.05 | 941.48 | 8079.57 / 4000 | 179.00 | 455.66 |
| 3840x2160-8cam-10hz-zstd-64MiB-jpeg | scan / forward | ok, ok, ok | 65.53 | 941.76 | 1329.97 / 4000 | 0.00 | 199.67 |
| 3840x2160-8cam-10hz-zstd-64MiB-jpeg | all / forward | ok, ok, ok | 65.53 | 941.76 | 8325.87 / 4000 | 179.00 | 565.79 |
| 3840x2160-8cam-10hz-zstd-128MiB-jpeg | scan / forward | product_limit, product_limit, product_limit | 130.80 | — | — / 4000 | — | 264.20 |
| 3840x2160-8cam-10hz-zstd-128MiB-jpeg | all / forward | product_limit, product_limit, product_limit | 130.80 | — | — / 4000 | — | 501.71 |
| 3840x2160-8cam-10hz-zstd-256MiB-jpeg | scan / forward | product_limit, product_limit, product_limit | 258.32 | — | — / 4000 | — | 3.49 |
| 3840x2160-8cam-10hz-zstd-256MiB-jpeg | all / forward | product_limit, product_limit, product_limit | 258.32 | — | — / 4000 | — | 3.48 |
| 3840x2160-8cam-10hz-zstd-512MiB-jpeg | scan / forward | product_limit, product_limit, product_limit | 513.84 | — | — / 4000 | — | 3.48 |
| 3840x2160-8cam-10hz-zstd-512MiB-jpeg | all / forward | product_limit, product_limit, product_limit | 513.84 | — | — / 4000 | — | 3.47 |
| 3840x2160-8cam-10hz-zstd-512MiB-jpeg | scan / indexed | product_limit, product_limit, product_limit | 513.84 | — | — / 4000 | — | 3.47 |
| 3840x2160-8cam-10hz-lz4-64MiB-jpeg | scan / forward | ok, ok, ok | 65.53 | 943.49 | 1514.76 / 4000 | 0.00 | 199.46 |
| 3840x2160-8cam-10hz-lz4-64MiB-jpeg | all / forward | ok, ok, ok | 65.53 | 943.49 | 8602.56 / 4000 | 179.00 | 565.80 |
| 3840x2160-8cam-10hz-lz4-512MiB-jpeg | scan / forward | product_limit, product_limit, product_limit | 513.84 | — | — / 4000 | — | 3.46 |
| 3840x2160-8cam-10hz-lz4-512MiB-jpeg | all / forward | product_limit, product_limit, product_limit | 513.84 | — | — / 4000 | — | 3.42 |
| 3840x2160-8cam-10hz-lz4-512MiB-jpeg | scan / indexed | product_limit, product_limit, product_limit | 513.84 | — | — / 4000 | — | 3.46 |
| 3840x2160-4cam-20hz-none-64MiB-jpeg | scan / forward | ok, ok, ok | 65.53 | 950.15 | 693.36 / 4000 | 0.00 | 134.33 |
| 3840x2160-4cam-20hz-none-64MiB-jpeg | all / forward | ok, ok, ok | 65.53 | 950.15 | 4676.29 / 4000 | 100.00 | 406.30 |
| 3840x2160-4cam-20hz-zstd-64MiB-jpeg | scan / forward | ok, ok, ok | 65.53 | 941.68 | 1308.92 / 4000 | 0.00 | 199.29 |
| 3840x2160-4cam-20hz-zstd-64MiB-jpeg | all / forward | ok, ok, ok | 65.53 | 941.68 | 5332.90 / 4000 | 100.00 | 439.93 |
| 3840x2160-8cam-10hz-zstd-512MiB-grouped-jpeg | scan / forward | ok, ok, ok | 29.78 | 0.01 | 1.35 / 4000 | 0.00 | 4.18 |
| 3840x2160-8cam-10hz-zstd-512MiB-grouped-jpeg | all / forward | ok, ok, ok | 29.78 | 941.75 | 8092.36 / 4000 | 179.00 | 821.82 |
| 3840x2160-8cam-10hz-none-64MiB-raw | scan / forward | ok, ok, ok | 71.19 | 711.94 | 489.61 / 1000 | 0.00 | 145.83 |
| 3840x2160-8cam-10hz-none-64MiB-raw | all / forward | product_limit, product_limit, product_limit | 71.19 | — | — / 1000 | — | 217.02 |
| 3840x2160-8cam-10hz-none-128MiB-raw | scan / forward | product_limit, product_limit, product_limit | 142.39 | — | — / 1000 | — | 145.63 |
| 3840x2160-8cam-10hz-none-128MiB-raw | all / forward | product_limit, product_limit, product_limit | 142.39 | — | — / 1000 | — | 145.71 |
| 3840x2160-8cam-10hz-none-512MiB-raw | scan / forward | product_limit, product_limit, product_limit | 522.08 | — | — / 1000 | — | 3.45 |
| 3840x2160-8cam-10hz-none-512MiB-raw | all / forward | product_limit, product_limit, product_limit | 522.08 | — | — / 1000 | — | 3.48 |
| 3840x2160-8cam-10hz-none-512MiB-raw | scan / indexed | product_limit, product_limit, product_limit | 522.08 | — | — / 1000 | — | 3.51 |
| 320x240-8cam-10hz-zstd-64MiB-jpeg | scan / forward | ok, ok, ok | 8.96 | 1.92 | 6.55 / 4000 | 0.00 | 14.70 |
| 320x240-8cam-10hz-zstd-64MiB-jpeg | all / forward | ok, ok, ok | 8.96 | 1.92 | 80.73 / 4000 | 179.00 | 16.51 |
