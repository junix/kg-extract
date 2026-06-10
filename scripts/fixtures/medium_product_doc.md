# Aurora Portal Architecture Note

Aurora Portal is a customer operations product developed by Helio Systems.
Helio Systems is located in Singapore.

## Core Modules

Aurora Portal includes the Identity Service, the Billing Worker, the Audit Console, and the Export Job.
The API Gateway requires the Identity Service before it accepts customer requests.

## Storage and Messaging

The Identity Service uses PostgreSQL for user profiles and session records.
The Billing Worker uses Kafka for asynchronous billing messages.
The Audit Console uses ClickHouse for analytical audit queries.
The Export Job uses Object Storage for generated CSV exports.

## Runtime

Aurora Portal is deployed in Kubernetes.
The Billing Worker produces Invoice Events after invoice calculations finish.

## Governance

The Data Retention Policy governs Audit Logs.
Incident Reports are evidenced by Audit Logs.
